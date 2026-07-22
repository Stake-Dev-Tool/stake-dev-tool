//! Workspace realtime fan-out: the per-workspace broadcast channels that back
//! the SSE stream (`GET /api/workspaces/:slug/events`). Kept in its own module
//! (rather than inside `api::documents`) so that both the document handlers and
//! `api::math::create_revision` can publish without a circular dependency, and
//! so `AppState` can hold the hub.

use axum::response::sse::Event;
use dashmap::DashMap;
use protocol::{DocumentEvent, FrontPushedEvent, RevisionPushedEvent};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Per-workspace channel capacity. A subscriber that falls this far behind gets
/// a `Lagged` signal; the SSE stream skips the gap and the client resyncs via
/// `?since_seq=` rather than the stream closing.
const CHANNEL_CAPACITY: usize = 256;

/// One realtime event fanned out to a workspace's SSE subscribers. Payloads are
/// deliberately minimal — the client pulls the actual document/revision over
/// HTTP after being nudged.
///
/// `membership` events are specced in the contract but deferred in M3: the only
/// membership mutation this milestone owns is delete-workspace (which removes
/// the whole channel anyway), and invite-accept lives in `api::invites`, which
/// M3 must not touch.
#[derive(Debug, Clone)]
pub enum WorkspaceEvent {
    Document(DocumentEvent),
    RevisionPushed(RevisionPushedEvent),
    FrontPushed(FrontPushedEvent),
}

impl WorkspaceEvent {
    /// Render to a named SSE frame. These tiny structs always serialize, so a
    /// (theoretically impossible) failure degrades to an empty comment frame
    /// rather than tearing down the stream.
    pub fn to_sse_event(&self) -> Event {
        let named = match self {
            WorkspaceEvent::Document(d) => Event::default().event("document").json_data(d),
            WorkspaceEvent::RevisionPushed(r) => {
                Event::default().event("revision_pushed").json_data(r)
            }
            WorkspaceEvent::FrontPushed(f) => Event::default().event("front_pushed").json_data(f),
        };
        named.unwrap_or_else(|_| Event::default().comment(""))
    }
}

/// The set of live per-workspace broadcast channels. Senders are created lazily
/// on first subscribe and reclaimed once their last receiver drops (detected on
/// the next publish), so the map never accumulates idle channels.
#[derive(Default)]
pub struct WorkspaceEvents {
    channels: DashMap<Uuid, broadcast::Sender<WorkspaceEvent>>,
}

impl WorkspaceEvents {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to a workspace's stream, creating its channel if needed.
    pub fn subscribe(&self, workspace_id: Uuid) -> broadcast::Receiver<WorkspaceEvent> {
        self.channels
            .entry(workspace_id)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .subscribe()
    }

    /// Publish an event to a workspace's subscribers. A send with no receivers
    /// means every SSE client has disconnected, so the now-idle sender is
    /// dropped (re-checked under the removal guard so a subscriber that just
    /// arrived is never evicted).
    pub fn publish(&self, workspace_id: Uuid, event: WorkspaceEvent) {
        let idle = match self.channels.get(&workspace_id) {
            Some(sender) => sender.send(event).is_err(),
            None => false,
        };
        if idle {
            self.channels
                .remove_if(&workspace_id, |_, s| s.receiver_count() == 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_reaches_a_subscriber_and_idle_channels_are_reclaimed() {
        let hub = WorkspaceEvents::new();
        let ws = Uuid::new_v4();

        let mut rx = hub.subscribe(ws);
        hub.publish(
            ws,
            WorkspaceEvent::RevisionPushed(RevisionPushedEvent {
                game: "g".to_string(),
                number: 1,
            }),
        );
        let got = rx.recv().await.expect("event delivered");
        assert!(matches!(got, WorkspaceEvent::RevisionPushed(_)));

        // Dropping the last receiver leaves the sender until the next publish
        // notices it has no listeners and reclaims it.
        drop(rx);
        assert_eq!(hub.channels.len(), 1);
        hub.publish(
            ws,
            WorkspaceEvent::RevisionPushed(RevisionPushedEvent {
                game: "g".to_string(),
                number: 2,
            }),
        );
        assert_eq!(hub.channels.len(), 0, "idle channel reclaimed on publish");
    }
}
