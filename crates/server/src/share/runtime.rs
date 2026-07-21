//! Process-local in-memory state for the public share path: visitor-session
//! tracking (for the per-link concurrency cap + lifetime counts), unlock tokens
//! for password-protected links, and a per-`(link, IP)` session-creation rate
//! limiter.
//!
//! All of this is intentionally node-local (like [`crate::auth::ratelimit`]): a
//! multi-node deployment would under-count and let the cap drift by `nodes ×`,
//! which is fine for best-effort analytics and abuse-limiting. Everything is
//! keyed by a share-link `Uuid` (unique per link) so concurrent integration
//! tests never collide.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use rand::RngCore;
use uuid::Uuid;

/// A visitor session is considered live for 30 min after its last wallet call
/// (sliding). Stale entries are purged lazily on the next touch of that link.
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);
/// Unlock cookies live 12 h.
const UNLOCK_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// Session-creation rate limit: at most this many *new* sessions per window per
/// `(link, IP)`.
const RL_MAX_NEW_SESSIONS: u32 = 60;
const RL_WINDOW: Duration = Duration::from_secs(60);

static RUNTIME: OnceLock<ShareRuntime> = OnceLock::new();

/// The process-global share runtime.
pub(super) fn runtime() -> &'static ShareRuntime {
    RUNTIME.get_or_init(ShareRuntime::new)
}

/// Outcome of noting a visitor session against a link's concurrency cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Admit {
    /// A session id seen before on this link; last-seen refreshed.
    Existing,
    /// A brand-new session id, admitted (caller bumps `sessions_count`).
    Created,
    /// A brand-new session id rejected because the link is at its cap.
    OverCap,
}

struct Unlock {
    share_id: Uuid,
    created: Instant,
}

struct RlWindow {
    started: Instant,
    count: u32,
}

pub(super) struct ShareRuntime {
    /// share_id -> (visitor session id -> last_seen).
    sessions: DashMap<Uuid, Arc<DashMap<String, Instant>>>,
    /// unlock cookie token -> which link it unlocks + when it was minted.
    unlocks: DashMap<String, Unlock>,
    /// (share_id, client ip) -> new-session window.
    ratelimit: DashMap<(Uuid, String), RlWindow>,
}

impl ShareRuntime {
    fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            unlocks: DashMap::new(),
            ratelimit: DashMap::new(),
        }
    }

    fn sessions_for(&self, share_id: Uuid) -> Arc<DashMap<String, Instant>> {
        self.sessions
            .entry(share_id)
            .or_insert_with(|| Arc::new(DashMap::new()))
            .clone()
    }

    /// Note a wallet-carrying session id against `share_id`'s cap. Purges stale
    /// entries first, then classifies the id as existing / newly-created /
    /// over-cap. `max` <= 0 is treated as unlimited.
    pub(super) fn note_session(&self, share_id: Uuid, session_id: &str, max: i32) -> Admit {
        let sessions = self.sessions_for(share_id);
        sessions.retain(|_, seen| seen.elapsed() < SESSION_TTL);

        if let Some(mut entry) = sessions.get_mut(session_id) {
            *entry = Instant::now();
            return Admit::Existing;
        }
        if max > 0 && sessions.len() >= max as usize {
            return Admit::OverCap;
        }
        sessions.insert(session_id.to_string(), Instant::now());
        Admit::Created
    }

    /// Live (non-stale) visitor-session count for a link, for the dashboard.
    pub(super) fn active_sessions(&self, share_id: Uuid) -> usize {
        match self.sessions.get(&share_id) {
            Some(sessions) => {
                sessions.retain(|_, seen| seen.elapsed() < SESSION_TTL);
                sessions.len()
            }
            None => 0,
        }
    }

    /// Fixed-window per-`(link, IP)` limiter for *new* session creation. Returns
    /// `true` when the attempt is within budget (and records it).
    pub(super) fn allow_new_session(&self, share_id: Uuid, ip: &str) -> bool {
        let mut window = self
            .ratelimit
            .entry((share_id, ip.to_string()))
            .or_insert_with(|| RlWindow {
                started: Instant::now(),
                count: 0,
            });
        if window.started.elapsed() >= RL_WINDOW {
            window.started = Instant::now();
            window.count = 0;
        }
        if window.count >= RL_MAX_NEW_SESSIONS {
            return false;
        }
        window.count += 1;
        true
    }

    /// Mint a random unlock token for `share_id` and store it (12 h TTL).
    pub(super) fn store_unlock(&self, share_id: Uuid) -> String {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = hex32(&bytes);
        self.unlocks.insert(
            token.clone(),
            Unlock {
                share_id,
                created: Instant::now(),
            },
        );
        token
    }

    /// True when `token` is a live unlock for exactly `share_id`.
    ///
    /// The read guard from `get` must be fully dropped before any `remove` on
    /// the same map — DashMap deadlocks on same-shard re-entry. And only
    /// *expired* tokens are removed: a token probed against the wrong link
    /// stays valid for its own link.
    pub(super) fn is_unlocked(&self, token: &str, share_id: Uuid) -> bool {
        let expired = match self.unlocks.get(token) {
            None => return false,
            Some(u) => {
                if u.created.elapsed() < UNLOCK_TTL {
                    return u.share_id == share_id;
                }
                true
            }
        };
        if expired {
            self.unlocks.remove(token);
        }
        false
    }
}

fn hex32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_admits_up_to_limit_then_rejects_new() {
        let rt = ShareRuntime::new();
        let id = Uuid::new_v4();
        assert_eq!(rt.note_session(id, "a", 2), Admit::Created);
        assert_eq!(rt.note_session(id, "b", 2), Admit::Created);
        // Existing ids never count against the cap.
        assert_eq!(rt.note_session(id, "a", 2), Admit::Existing);
        // A third distinct id is over the cap.
        assert_eq!(rt.note_session(id, "c", 2), Admit::OverCap);
        assert_eq!(rt.active_sessions(id), 2);
    }

    #[test]
    fn zero_or_negative_cap_is_unlimited() {
        let rt = ShareRuntime::new();
        let id = Uuid::new_v4();
        for i in 0..100 {
            assert_eq!(rt.note_session(id, &format!("s{i}"), 0), Admit::Created);
        }
    }

    #[test]
    fn unlock_tokens_scope_to_their_link() {
        let rt = ShareRuntime::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let token = rt.store_unlock(a);
        assert!(rt.is_unlocked(&token, a));
        assert!(!rt.is_unlocked(&token, b));
        assert!(!rt.is_unlocked("not-a-token", a));
    }

    #[test]
    fn rate_limit_blocks_past_budget() {
        let rt = ShareRuntime::new();
        let id = Uuid::new_v4();
        for _ in 0..RL_MAX_NEW_SESSIONS {
            assert!(rt.allow_new_session(id, "1.2.3.4"));
        }
        assert!(!rt.allow_new_session(id, "1.2.3.4"));
        // A different IP has its own budget.
        assert!(rt.allow_new_session(id, "5.6.7.8"));
    }
}
