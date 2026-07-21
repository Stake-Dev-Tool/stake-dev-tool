//! Workspace SSE subscription (`GET /api/workspaces/:slug/events`).
//!
//! A `reqwest` byte-stream is fed through a small `text/event-stream` line
//! parser into typed [`WorkspaceEvent`]s, which are forwarded to the frontend
//! as a Tauri event (`cloud-workspace-event`) so the UI reacts live. The
//! connection reconnects with capped backoff; on every (re)connect a
//! [`WorkspaceEvent::Reconnected`] is emitted first so the UI can pull
//! `?since_seq=<last known>` before it starts streaming (the contract's
//! reconnect protocol — clients do not rely on Last-Event-ID replay).

use futures_util::StreamExt;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use super::http::Conn;

/// Tauri event name the UI listens on.
pub const EVENT_NAME: &str = "cloud-workspace-event";

/// A typed workspace event forwarded to the frontend. Serialized with an
/// internal `type` tag so the UI can `switch` on it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEvent {
    /// The stream (re)connected — the UI should pull `?since_seq=<last>`.
    Reconnected { slug: String },
    /// A document changed; data is not inlined, the client pulls it.
    Document {
        slug: String,
        #[serde(rename = "docKind")]
        doc_kind: String,
        #[serde(rename = "docId")]
        doc_id: String,
        seq: i64,
    },
    /// A new math revision was committed.
    RevisionPushed {
        slug: String,
        game: String,
        number: i64,
    },
}

// ---------------------------------------------------------------------------
// Line parser
// ---------------------------------------------------------------------------

/// Incremental `text/event-stream` parser. Feed it decoded text chunks; it
/// buffers across chunk boundaries and returns one parsed event per completed
/// block (blank-line terminated). Keeps only the fields M3 uses (`event:` and
/// `data:`); comment lines (`:` prefix, e.g. the 25s keep-alive) are ignored.
#[derive(Default)]
pub struct SseParser {
    buf: String,
    event: Option<String>,
    data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a text chunk, returning every fully-parsed block it completed.
    /// `slug` stamps each event so a listener can tell workspaces apart.
    pub fn feed(&mut self, chunk: &str, slug: &str) -> Vec<WorkspaceEvent> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        // Process complete lines; keep any trailing partial line in `buf`.
        while let Some(nl) = self.buf.find('\n') {
            let mut line = self.buf[..nl].to_string();
            self.buf.drain(..=nl);
            // Tolerate CRLF.
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                if let Some(ev) = self.take_block(slug) {
                    out.push(ev);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                self.event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                // A single leading space after the colon is part of the format.
                self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
            // Any other field (`id:`, `retry:`, `:comment`) is ignored.
        }
        out
    }

    /// Emits the accumulated block (if any) and resets for the next one.
    fn take_block(&mut self, slug: &str) -> Option<WorkspaceEvent> {
        let event = self.event.take();
        let data = std::mem::take(&mut self.data);
        let event = event?;
        parse_block(&event, &data, slug)
    }
}

/// Turns one `(event, data)` pair into a typed [`WorkspaceEvent`]. Returns
/// `None` for unknown event names or undecodable data (forward-compatible).
fn parse_block(event: &str, data: &str, slug: &str) -> Option<WorkspaceEvent> {
    let json: serde_json::Value = serde_json::from_str(data).ok()?;
    match event {
        "document" => Some(WorkspaceEvent::Document {
            slug: slug.to_string(),
            doc_kind: json.get("kind")?.as_str()?.to_string(),
            doc_id: json.get("doc_id")?.as_str()?.to_string(),
            seq: json.get("seq")?.as_i64()?,
        }),
        "revision_pushed" => Some(WorkspaceEvent::RevisionPushed {
            slug: slug.to_string(),
            game: json.get("game")?.as_str()?.to_string(),
            number: json.get("number")?.as_i64()?,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Subscribe loop
// ---------------------------------------------------------------------------

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Runs the reconnecting subscription until the task is aborted. Each event is
/// handed to `on_event`; a [`WorkspaceEvent::Reconnected`] is delivered on every
/// successful (re)connect before any stream events.
pub async fn run<F: FnMut(WorkspaceEvent)>(slug: String, mut on_event: F) {
    let mut backoff = BACKOFF_MIN;
    loop {
        match connect_once(&slug, &mut on_event).await {
            Ok(()) => {
                // Stream ended cleanly (server closed); reconnect promptly.
                backoff = BACKOFF_MIN;
            }
            Err(e) => {
                tracing::warn!(slug = %slug, error = %e, "workspace SSE dropped; backing off");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One connection attempt: opens the stream, emits `Reconnected`, then parses
/// bytes until the stream ends or errors.
async fn connect_once<F: FnMut(WorkspaceEvent)>(
    slug: &str,
    on_event: &mut F,
) -> anyhow::Result<()> {
    let conn = Conn::connect()?.ok_or_else(|| anyhow::anyhow!("not signed in to cloud"))?;
    let path = format!("/api/workspaces/{slug}/events");
    let res = conn
        .request(reqwest::Method::GET, &path)
        .header("Accept", "text/event-stream")
        .send()
        .await?;
    if !res.status().is_success() {
        anyhow::bail!("workspace events HTTP {}", res.status());
    }

    // Connected: tell the UI to reconcile via ?since_seq before streaming.
    on_event(WorkspaceEvent::Reconnected {
        slug: slug.to_string(),
    });

    let mut parser = SseParser::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        // The stream is UTF-8; tolerate a chunk splitting a codepoint by using
        // lossy decode (control/ascii framing bytes are never split-sensitive).
        let text = String::from_utf8_lossy(&bytes);
        for ev in parser.feed(&text, slug) {
            on_event(ev);
        }
    }
    Ok(())
}

/// Spawns [`run`] as a background task that forwards every event to the
/// frontend via the `cloud-workspace-event` Tauri event. The returned handle is
/// stored by the command layer so a workspace switch can abort the previous
/// subscription.
pub fn spawn(app: AppHandle, slug: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run(slug, move |ev| {
            if let Err(e) = app.emit(EVENT_NAME, &ev) {
                tracing::warn!(error = %e, "failed to emit cloud-workspace-event");
            }
        })
        .await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_document_event_block() {
        let mut p = SseParser::new();
        let stream =
            "event: document\ndata: {\"kind\":\"saved_round\",\"doc_id\":\"r1\",\"seq\":42}\n\n";
        let evs = p.feed(stream, "team-a");
        assert_eq!(
            evs,
            vec![WorkspaceEvent::Document {
                slug: "team-a".into(),
                doc_kind: "saved_round".into(),
                doc_id: "r1".into(),
                seq: 42,
            }]
        );
    }

    #[test]
    fn parses_revision_pushed_and_ignores_keepalive_comment() {
        let mut p = SseParser::new();
        // A keep-alive comment line, then a real event.
        let evs = p.feed(": keep-alive\n\n", "w");
        assert!(evs.is_empty());
        let evs = p.feed(
            "event: revision_pushed\ndata: {\"game\":\"dice\",\"number\":3}\n\n",
            "w",
        );
        assert_eq!(
            evs,
            vec![WorkspaceEvent::RevisionPushed {
                slug: "w".into(),
                game: "dice".into(),
                number: 3,
            }]
        );
    }

    #[test]
    fn handles_events_split_across_chunk_boundaries() {
        let mut p = SseParser::new();
        // Feed the same event byte-fragmented across three chunks.
        assert!(p.feed("event: docum", "w").is_empty());
        assert!(p.feed("ent\ndata: {\"kind\":\"profile\",", "w").is_empty());
        let evs = p.feed("\"doc_id\":\"p9\",\"seq\":7}\n\n", "w");
        assert_eq!(
            evs,
            vec![WorkspaceEvent::Document {
                slug: "w".into(),
                doc_kind: "profile".into(),
                doc_id: "p9".into(),
                seq: 7,
            }]
        );
    }

    #[test]
    fn ignores_unknown_event_names_and_bad_json() {
        let mut p = SseParser::new();
        assert!(
            p.feed("event: membership\ndata: {\"x\":1}\n\n", "w")
                .is_empty()
        );
        assert!(
            p.feed("event: document\ndata: not-json\n\n", "w")
                .is_empty()
        );
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let mut p = SseParser::new();
        let s = "event: document\ndata: {\"kind\":\"profile\",\"doc_id\":\"a\",\"seq\":1}\n\n\
                 event: document\ndata: {\"kind\":\"saved_round\",\"doc_id\":\"b\",\"seq\":2}\n\n";
        let evs = p.feed(s, "w");
        assert_eq!(evs.len(), 2);
        assert_eq!(
            evs[1],
            WorkspaceEvent::Document {
                slug: "w".into(),
                doc_kind: "saved_round".into(),
                doc_id: "b".into(),
                seq: 2,
            }
        );
    }
}
