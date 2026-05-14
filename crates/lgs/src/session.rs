use crate::config::{self, intern_currency};
use crate::types::{EventEntry, Round, Session};
use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
use parking_lot::Mutex;
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const HISTORY_CAP: usize = 100;
const EVENT_CHANNEL_CAP: usize = 64;

pub struct SessionInit {
    pub game: String,
    pub language: Option<String>,
    pub balance: Option<u64>,
    pub currency: Option<&'static str>,
}

pub struct SessionStore {
    sessions: DashMap<String, Session>,
    bet_counter: AtomicU64,
    event_channels: DashMap<String, broadcast::Sender<EventEntry>>,
    storage: Option<SessionDb>,
}

impl SessionStore {
    pub fn new() -> Self {
        match SessionDb::open_default() {
            Ok(storage) => Self::from_storage(Some(storage)),
            Err(err) => {
                tracing::warn!(error = %err, "session sqlite storage unavailable; using memory only");
                Self::in_memory()
            }
        }
    }

    pub fn in_memory() -> Self {
        Self {
            sessions: DashMap::new(),
            bet_counter: AtomicU64::new(0),
            event_channels: DashMap::new(),
            storage: None,
        }
    }

    pub fn with_path(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::from_storage(Some(SessionDb::open(path)?)))
    }

    fn from_storage(storage: Option<SessionDb>) -> Self {
        let sessions = DashMap::new();
        let mut max_bet_id = 0;

        if let Some(db) = storage.as_ref() {
            match db.load_sessions() {
                Ok(loaded) => {
                    for session in loaded {
                        if let Some(round) = session.active_round.as_ref() {
                            max_bet_id = max_bet_id.max(round.bet_id);
                        }
                        sessions.insert(session.id.clone(), session);
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to load persisted sessions");
                }
            }
        }

        Self {
            sessions,
            bet_counter: AtomicU64::new(max_bet_id),
            event_channels: DashMap::new(),
            storage,
        }
    }

    /// Subscribe to new events for the given session. Lazily creates the
    /// broadcast channel on first subscribe. Multiple subscribers share the
    /// same channel (Stream of each new pushed EventEntry).
    pub fn subscribe_events(&self, session_id: &str) -> broadcast::Receiver<EventEntry> {
        self.event_channels
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(EVENT_CHANNEL_CAP).0)
            .subscribe()
    }

    pub fn create(&self, session_id: &str, game: &str, language: Option<String>) -> Session {
        self.upsert(
            session_id,
            SessionInit {
                game: game.to_string(),
                language,
                balance: None,
                currency: None,
            },
        )
    }

    /// Insert or replace a session with the given init params. Called from
    /// the devtool's `/sessions/prepare` endpoint so the test view can stage
    /// balance/currency/language before the game calls `/authenticate`.
    pub fn upsert(&self, session_id: &str, init: SessionInit) -> Session {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let session = Session {
            id: session_id.to_string(),
            game: init.game,
            balance: init.balance.unwrap_or(config::INITIAL_BALANCE),
            currency: init.currency.unwrap_or(config::CURRENCY),
            language: init
                .language
                .unwrap_or_else(|| config::LANGUAGE.to_string()),
            active_round: None,
            created_at: now,
            last_event_id: None,
            last_payout_multiplier: None,
            event_history: Vec::new(),
        };
        self.sessions
            .insert(session_id.to_string(), session.clone());
        self.persist_session(&session);
        session
    }

    /// Create a missing session, or refresh metadata for an existing one
    /// without wiping balance, pending round, or event history.
    pub fn prepare(&self, session_id: &str, init: SessionInit) -> Session {
        if let Some(mut entry) = self.sessions.get_mut(session_id) {
            entry.game = init.game;
            if let Some(language) = init.language {
                entry.language = language;
            }
            if let Some(currency) = init.currency {
                entry.currency = currency;
            }
            let session = entry.clone();
            drop(entry);
            self.persist_session(&session);
            return session;
        }

        self.upsert(session_id, init)
    }

    pub fn set_last_event(
        &self,
        session_id: &str,
        event_id: u32,
        payout_multiplier: u32,
    ) -> Option<Session> {
        let session = {
            let mut entry = self.sessions.get_mut(session_id)?;
            entry.last_event_id = Some(event_id);
            entry.last_payout_multiplier = Some(payout_multiplier);
            entry.clone()
        };
        self.persist_session(&session);
        Some(session)
    }

    /// Push an event entry into the session's history (most-recent-first).
    /// Capped at `HISTORY_CAP` — older entries are dropped. Also broadcasts
    /// the entry to any active SSE subscribers.
    pub fn push_event(&self, session_id: &str, entry: EventEntry) -> Option<Session> {
        let session = {
            let mut s = self.sessions.get_mut(session_id)?;
            s.event_history.insert(0, entry.clone());
            if s.event_history.len() > HISTORY_CAP {
                s.event_history.truncate(HISTORY_CAP);
            }
            s.clone()
        };
        if let Some(tx) = self.event_channels.get(session_id) {
            let _ = tx.send(entry);
        }
        self.persist_session(&session);
        Some(session)
    }

    /// Fetch existing session, or create with defaults. Used by authenticate
    /// so a pre-configured session (set via Tauri prepare_session) is preserved.
    pub fn get_or_create(&self, session_id: &str, game: &str, language: Option<String>) -> Session {
        if let Some(s) = self.sessions.get(session_id) {
            return s.clone();
        }
        self.create(session_id, game, language)
    }

    pub fn get(&self, session_id: &str) -> Option<Session> {
        self.sessions.get(session_id).map(|s| s.clone())
    }

    pub fn set_active_round(&self, session_id: &str, round: Option<Round>) -> Option<Session> {
        let session = {
            let mut entry = self.sessions.get_mut(session_id)?;
            entry.active_round = round;
            entry.clone()
        };
        self.persist_session(&session);
        Some(session)
    }

    pub fn deduct_bet(&self, session_id: &str, amount: u64) -> Option<Session> {
        let session = {
            let mut entry = self.sessions.get_mut(session_id)?;
            if entry.balance < amount {
                return None;
            }
            entry.balance -= amount;
            entry.clone()
        };
        self.persist_session(&session);
        Some(session)
    }

    pub fn add_winnings(&self, session_id: &str, amount: u64) -> Option<Session> {
        let session = {
            let mut entry = self.sessions.get_mut(session_id)?;
            entry.balance = entry.balance.saturating_add(amount);
            entry.clone()
        };
        self.persist_session(&session);
        Some(session)
    }

    pub fn next_bet_id(&self) -> u64 {
        self.bet_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn reset_all(&self) -> Result<()> {
        self.sessions.clear();
        self.event_channels.clear();
        if let Some(storage) = self.storage.as_ref() {
            storage.clear()?;
        }
        Ok(())
    }

    fn persist_session(&self, session: &Session) {
        if let Some(storage) = self.storage.as_ref()
            && let Err(err) = storage.save_session(session)
        {
            tracing::warn!(session_id = %session.id, error = %err, "failed to persist session");
        }
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

struct SessionDb {
    conn: Mutex<Connection>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredRound {
    bet_id: u64,
    amount: u64,
    payout: u64,
    payout_multiplier: f64,
    active: bool,
    mode: String,
    event: Option<String>,
    state: String,
}

impl SessionDb {
    fn open_default() -> Result<Self> {
        let path = sessions_db_path()?;
        Self::open(path)
    }

    fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create session db dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open session db {}", path.display()))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY NOT NULL,
                game TEXT NOT NULL,
                balance INTEGER NOT NULL,
                currency TEXT NOT NULL,
                language TEXT NOT NULL,
                active_round TEXT,
                created_at INTEGER NOT NULL,
                last_event_id INTEGER,
                last_payout_multiplier INTEGER,
                event_history TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .context("migrate session db")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn load_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, game, balance, currency, language, active_round,
                       created_at, last_event_id, last_payout_multiplier, event_history
                FROM sessions
                "#,
            )
            .context("prepare load sessions")?;
        let mut rows = stmt.query([]).context("query sessions")?;
        let mut sessions = Vec::new();
        while let Some(row) = rows.next().context("read session row")? {
            match session_from_row(row) {
                Ok(session) => sessions.push(session),
                Err(err) => tracing::warn!(error = %err, "skipping invalid persisted session"),
            }
        }
        Ok(sessions)
    }

    fn save_session(&self, session: &Session) -> Result<()> {
        let active_round = session
            .active_round
            .as_ref()
            .map(stored_round_json)
            .transpose()?;
        let event_history =
            serde_json::to_string(&session.event_history).context("serialize event history")?;
        let now = now_ms();
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO sessions (
                id, game, balance, currency, language, active_round, created_at,
                last_event_id, last_payout_multiplier, event_history, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(id) DO UPDATE SET
                game = excluded.game,
                balance = excluded.balance,
                currency = excluded.currency,
                language = excluded.language,
                active_round = excluded.active_round,
                created_at = excluded.created_at,
                last_event_id = excluded.last_event_id,
                last_payout_multiplier = excluded.last_payout_multiplier,
                event_history = excluded.event_history,
                updated_at = excluded.updated_at
            "#,
            params![
                session.id,
                session.game,
                u64_to_i64(session.balance),
                session.currency,
                session.language,
                active_round,
                u64_to_i64(session.created_at),
                session.last_event_id.map(i64::from),
                session.last_payout_multiplier.map(i64::from),
                event_history,
                u64_to_i64(now),
            ],
        )
        .with_context(|| format!("upsert session {}", session.id))?;
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM sessions", [])
            .context("delete sessions")?;
        Ok(())
    }
}

fn sessions_db_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("could not resolve local data dir"))?
        .join("stake-dev-tool");
    Ok(dir.join("sessions.sqlite3"))
}

fn session_from_row(row: &Row<'_>) -> Result<Session> {
    let id: String = row.get(0).context("id")?;
    let game: String = row.get(1).context("game")?;
    let balance: i64 = row.get(2).context("balance")?;
    let currency: String = row.get(3).context("currency")?;
    let language: String = row.get(4).context("language")?;
    let active_round_json: Option<String> = row.get(5).context("active_round")?;
    let created_at: i64 = row.get(6).context("created_at")?;
    let last_event_id: Option<i64> = row.get(7).context("last_event_id")?;
    let last_payout_multiplier: Option<i64> = row.get(8).context("last_payout_multiplier")?;
    let event_history_json: String = row.get(9).context("event_history")?;

    let mut event_history: Vec<EventEntry> =
        serde_json::from_str(&event_history_json).context("parse event history")?;
    if event_history.len() > HISTORY_CAP {
        event_history.truncate(HISTORY_CAP);
    }

    Ok(Session {
        id,
        game,
        balance: i64_to_u64(balance),
        currency: intern_currency(&currency),
        language,
        active_round: active_round_json
            .as_deref()
            .map(round_from_json)
            .transpose()?,
        created_at: i64_to_u64(created_at),
        last_event_id: last_event_id.and_then(i64_to_u32),
        last_payout_multiplier: last_payout_multiplier.and_then(i64_to_u32),
        event_history,
    })
}

fn stored_round_json(round: &Round) -> Result<String> {
    let stored = StoredRound {
        bet_id: round.bet_id,
        amount: round.amount,
        payout: round.payout,
        payout_multiplier: round.payout_multiplier,
        active: round.active,
        mode: round.mode.clone(),
        event: round.event.clone(),
        state: round.state.get().to_string(),
    };
    serde_json::to_string(&stored).context("serialize active round")
}

fn round_from_json(json: &str) -> Result<Round> {
    let stored: StoredRound = serde_json::from_str(json).context("parse active round")?;
    let state = RawValue::from_string(stored.state).context("parse active round state")?;
    Ok(Round {
        bet_id: stored.bet_id,
        amount: stored.amount,
        payout: stored.payout,
        payout_multiplier: stored.payout_multiplier,
        active: stored.active,
        mode: stored.mode,
        event: stored.event,
        state: Arc::from(state),
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn i64_to_u32(value: i64) -> Option<u32> {
    u32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_existing_session_without_prepare_wiping_history() {
        let path = std::env::temp_dir().join(format!(
            "stake-dev-tool-session-test-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));

        let store = SessionStore::with_path(&path).expect("open store");
        let sid = "session-a";
        store.prepare(
            sid,
            SessionInit {
                game: "game-a".to_string(),
                language: Some("en".to_string()),
                balance: Some(10_000),
                currency: Some("USD"),
            },
        );
        store.deduct_bet(sid, 250).expect("deduct");
        store
            .push_event(
                sid,
                EventEntry {
                    event_id: 42,
                    mode: "base".to_string(),
                    bet_amount: 250,
                    payout: 0,
                    payout_multiplier: 0,
                    forced: false,
                    at: 1,
                },
            )
            .expect("push event");
        drop(store);

        let store = SessionStore::with_path(&path).expect("reopen store");
        let loaded = store.get(sid).expect("loaded session");
        assert_eq!(loaded.balance, 9_750);
        assert_eq!(loaded.event_history.len(), 1);

        store.prepare(
            sid,
            SessionInit {
                game: "game-a".to_string(),
                language: Some("fr".to_string()),
                balance: Some(99_999),
                currency: Some("EUR"),
            },
        );
        let prepared = store.get(sid).expect("prepared session");
        assert_eq!(prepared.balance, 9_750);
        assert_eq!(prepared.language, "fr");
        assert_eq!(prepared.currency, "EUR");
        assert_eq!(prepared.event_history.len(), 1);

        store.reset_all().expect("reset");
        assert!(store.get(sid).is_none());
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
