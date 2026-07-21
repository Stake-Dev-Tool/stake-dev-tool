//! Client for the M3 versioned-document endpoints (`docs/v2/m3-contract.md`).
//!
//! Per the ownership note, these are **local serde mirror structs** built from
//! the contract, not imports of `protocol` (the M3 server types are being
//! authored concurrently). The field names/shapes match the ts-rs-exported
//! `ProfileDocument` / `SavedRoundDocument` / `DocumentEnvelope` so the wire
//! stays compatible.
//!
//! The push/pull orchestration ([`put_lww`]) is written against the
//! [`DocumentApi`] trait so its optimistic-concurrency retry is unit-tested
//! against a fake client — mirroring how `crates/cli` tests its push flow.

use lgs::settings::ResolutionPreset;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::http::Conn;

/// `kind` value for profile documents.
pub const KIND_PROFILE: &str = "profile";
/// `kind` value for saved-round (bookmark) documents.
pub const KIND_SAVED_ROUND: &str = "saved_round";

// ---------------------------------------------------------------------------
// Wire mirror structs
// ---------------------------------------------------------------------------

/// One document as returned by the server. `data` is left as an opaque
/// [`Value`] so a single envelope type serves every `kind`; the caller decodes
/// `data` into the matching typed payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentEnvelope {
    pub kind: String,
    pub doc_id: String,
    pub data: Value,
    pub revision: i64,
    pub seq: i64,
    #[serde(default)]
    pub updated_by_display: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

/// `GET …/documents?kind=&since_seq=` response.
#[derive(Debug, Clone, Deserialize)]
pub struct DocumentList {
    #[serde(default)]
    pub documents: Vec<DocumentEnvelope>,
    #[serde(default)]
    pub latest_seq: i64,
}

/// Success body of a `PUT`. Fields mirror the server response; the sync flow
/// only branches on success/conflict today, so they are not all consumed.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PutResponse {
    pub revision: i64,
    pub seq: i64,
}

/// Success body of a `DELETE` (mirrors the server; `seq` unused for now).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct DeleteResponse {
    pub seq: i64,
}

/// The `profile` document payload (the `data` blob). Mirrors the V1 `Profile`
/// minus the per-machine `gamePath` and legacy `teamId` (contract decision #3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDoc {
    pub name: String,
    pub game_slug: String,
    #[serde(default)]
    pub game: Option<String>,
    #[serde(default)]
    pub revision: Option<i64>,
    #[serde(default)]
    pub front_url: Option<String>,
    #[serde(default)]
    pub resolutions: Vec<ResolutionPreset>,
    pub created_at: u64,
}

/// The `saved_round` document payload. V1 `SavedRound` plus the optional M2
/// revision pin (`None` = "latest at the time"; legacy imports leave it null).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedRoundDoc {
    pub game_slug: String,
    pub mode: String,
    pub event_id: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub revision: Option<i64>,
    pub created_at: u64,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors surfaced by the document client. [`DocError::Conflict`] carries the
/// current server envelope so the caller can reconcile (client-driven LWW).
#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("not signed in to cloud")]
    NotSignedIn,
    /// 409 `document_conflict`: `base_revision` did not match. `current` is the
    /// server's live document (absent on a delete-vs-delete race).
    #[error("document conflict (server revision {server_revision})")]
    Conflict {
        server_revision: i64,
        current: Option<Box<DocumentEnvelope>>,
    },
    #[error("{code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("cloud request failed: HTTP {status} {body}")]
    Http { status: u16, body: String },
    #[error("failed to decode cloud response: {0}")]
    Decode(String),
    #[error("network error: {0}")]
    Transport(String),
    #[error("{0}")]
    Other(String),
}

/// Server error envelope, optionally carrying the `current` document on a 409.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: Option<ErrorDetail>,
    #[serde(default)]
    current: Option<DocumentEnvelope>,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

impl DocError {
    /// Maps a failed response body to a typed error, recognising the
    /// `document_conflict` shape (`{ error, current }`).
    fn from_response(status: u16, body: &str) -> Self {
        match serde_json::from_str::<ErrorBody>(body) {
            Ok(env) => {
                let detail = env.error.unwrap_or_default();
                if detail.code == "document_conflict" {
                    let server_revision = env.current.as_ref().map(|c| c.revision).unwrap_or(0);
                    return DocError::Conflict {
                        server_revision,
                        current: env.current.map(Box::new),
                    };
                }
                if detail.code.is_empty() && detail.message.is_empty() {
                    DocError::Http {
                        status,
                        body: body.to_string(),
                    }
                } else {
                    DocError::Api {
                        status,
                        code: detail.code,
                        message: detail.message,
                    }
                }
            }
            Err(_) => DocError::Http {
                status,
                body: body.to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The trait push/pull orchestration is written against
// ---------------------------------------------------------------------------

/// The document operations the sync flow needs, factored into a trait so the
/// optimistic-concurrency logic can be exercised offline with a fake client.
#[allow(async_fn_in_trait)]
pub trait DocumentApi {
    async fn list(
        &self,
        kind: Option<&str>,
        since_seq: Option<i64>,
    ) -> Result<DocumentList, DocError>;

    async fn put(
        &self,
        kind: &str,
        doc_id: &str,
        data: &Value,
        base_revision: Option<i64>,
    ) -> Result<PutResponse, DocError>;

    async fn delete(
        &self,
        kind: &str,
        doc_id: &str,
        base_revision: Option<i64>,
    ) -> Result<DeleteResponse, DocError>;
}

/// Client-driven last-write-wins PUT: attempt the write, and on a single
/// `document_conflict` adopt the server's current revision as the new
/// `base_revision` and retry **keeping our data** (contract §"keep mine").
///
/// `base_revision` is the revision we believe is current (`None` = we think the
/// document does not exist yet). Returns the applied `PutResponse` or the final
/// error if the conflict recurs.
pub async fn put_lww<A: DocumentApi>(
    api: &A,
    kind: &str,
    doc_id: &str,
    data: &Value,
    base_revision: Option<i64>,
) -> Result<PutResponse, DocError> {
    match api.put(kind, doc_id, data, base_revision).await {
        Ok(resp) => Ok(resp),
        Err(DocError::Conflict {
            server_revision, ..
        }) => {
            // Keep mine: retry once against the fresh server revision.
            api.put(kind, doc_id, data, Some(server_revision)).await
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// The real client
// ---------------------------------------------------------------------------

/// Bearer-authenticated document client bound to one workspace `slug`.
#[derive(Clone)]
pub struct CloudDocuments {
    conn: Conn,
    slug: String,
}

impl CloudDocuments {
    /// Builds a client from the stored token; `Err(NotSignedIn)` when signed out.
    pub fn new(slug: &str) -> Result<Self, DocError> {
        let conn = Conn::connect()
            .map_err(|e| DocError::Other(e.to_string()))?
            .ok_or(DocError::NotSignedIn)?;
        Ok(Self {
            conn,
            slug: slug.to_string(),
        })
    }

    async fn send_json<T: serde::de::DeserializeOwned>(
        rb: reqwest::RequestBuilder,
    ) -> Result<T, DocError> {
        let res = rb
            .send()
            .await
            .map_err(|e| DocError::Transport(e.to_string()))?;
        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| DocError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(DocError::from_response(status.as_u16(), &body));
        }
        serde_json::from_str(&body).map_err(|e| DocError::Decode(e.to_string()))
    }

    /// `GET …/documents/:kind/:doc_id` — one live envelope (404 → `None`).
    pub async fn get(
        &self,
        kind: &str,
        doc_id: &str,
    ) -> Result<Option<DocumentEnvelope>, DocError> {
        let path = format!("/api/workspaces/{}/documents/{kind}/{doc_id}", self.slug);
        let res = self
            .conn
            .request(reqwest::Method::GET, &path)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| DocError::Transport(e.to_string()))?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| DocError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(DocError::from_response(status.as_u16(), &body));
        }
        serde_json::from_str(&body)
            .map(Some)
            .map_err(|e| DocError::Decode(e.to_string()))
    }
}

impl DocumentApi for CloudDocuments {
    async fn list(
        &self,
        kind: Option<&str>,
        since_seq: Option<i64>,
    ) -> Result<DocumentList, DocError> {
        let path = format!("/api/workspaces/{}/documents", self.slug);
        let mut rb = self
            .conn
            .request(reqwest::Method::GET, &path)
            .header("Accept", "application/json");
        if let Some(k) = kind {
            rb = rb.query(&[("kind", k)]);
        }
        if let Some(s) = since_seq {
            rb = rb.query(&[("since_seq", s.to_string())]);
        }
        Self::send_json(rb).await
    }

    async fn put(
        &self,
        kind: &str,
        doc_id: &str,
        data: &Value,
        base_revision: Option<i64>,
    ) -> Result<PutResponse, DocError> {
        let path = format!("/api/workspaces/{}/documents/{kind}/{doc_id}", self.slug);
        let rb = self
            .conn
            .request(reqwest::Method::PUT, &path)
            .header("Accept", "application/json")
            .json(&serde_json::json!({ "data": data, "base_revision": base_revision }));
        Self::send_json(rb).await
    }

    async fn delete(
        &self,
        kind: &str,
        doc_id: &str,
        base_revision: Option<i64>,
    ) -> Result<DeleteResponse, DocError> {
        let path = format!("/api/workspaces/{}/documents/{kind}/{doc_id}", self.slug);
        let rb = self
            .conn
            .request(reqwest::Method::DELETE, &path)
            .header("Accept", "application/json")
            .json(&serde_json::json!({ "base_revision": base_revision }));
        Self::send_json(rb).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records PUT/DELETE calls and replays a scripted first response so the LWW
    /// retry can be tested offline (mirrors `crates/cli`'s `FakeClient`).
    struct FakeDocs {
        /// On the first PUT, return this conflict (with the given server rev).
        conflict_once: Mutex<Option<i64>>,
        put_calls: Mutex<Vec<Option<i64>>>, // base_revision of each PUT
    }

    impl FakeDocs {
        fn conflicting(server_rev: i64) -> Self {
            Self {
                conflict_once: Mutex::new(Some(server_rev)),
                put_calls: Mutex::new(Vec::new()),
            }
        }
        fn clean() -> Self {
            Self {
                conflict_once: Mutex::new(None),
                put_calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl DocumentApi for FakeDocs {
        async fn list(
            &self,
            _kind: Option<&str>,
            _since: Option<i64>,
        ) -> Result<DocumentList, DocError> {
            Ok(DocumentList {
                documents: vec![],
                latest_seq: 0,
            })
        }
        async fn put(
            &self,
            _kind: &str,
            _doc_id: &str,
            _data: &Value,
            base_revision: Option<i64>,
        ) -> Result<PutResponse, DocError> {
            self.put_calls.lock().unwrap().push(base_revision);
            let mut c = self.conflict_once.lock().unwrap();
            if let Some(server_rev) = c.take() {
                return Err(DocError::Conflict {
                    server_revision: server_rev,
                    current: None,
                });
            }
            Ok(PutResponse {
                revision: base_revision.unwrap_or(0) + 1,
                seq: 100,
            })
        }
        async fn delete(
            &self,
            _kind: &str,
            _doc_id: &str,
            _base: Option<i64>,
        ) -> Result<DeleteResponse, DocError> {
            Ok(DeleteResponse { seq: 1 })
        }
    }

    #[tokio::test]
    async fn put_lww_retries_once_on_conflict_keeping_local() {
        let fake = FakeDocs::conflicting(7);
        let data = serde_json::json!({ "name": "mine" });
        let resp = put_lww(&fake, KIND_PROFILE, "id1", &data, None)
            .await
            .unwrap();
        // Applied against the server's revision (7 → 8).
        assert_eq!(resp.revision, 8);
        let calls = fake.put_calls.lock().unwrap();
        // First with our believed base (None = create), then the server's rev.
        assert_eq!(*calls, vec![None, Some(7)]);
    }

    #[tokio::test]
    async fn put_lww_passes_through_when_no_conflict() {
        let fake = FakeDocs::clean();
        let data = serde_json::json!({ "x": 1 });
        let resp = put_lww(&fake, KIND_SAVED_ROUND, "id2", &data, Some(3))
            .await
            .unwrap();
        assert_eq!(resp.revision, 4);
        assert_eq!(*fake.put_calls.lock().unwrap(), vec![Some(3)]);
    }

    #[test]
    fn decodes_document_conflict_envelope() {
        let body = r#"{"error":{"code":"document_conflict","message":"stale"},
            "current":{"kind":"saved_round","doc_id":"r1","data":{},"revision":5,"seq":9,
            "updated_by_display":"alice","updated_at":null,"deleted":false}}"#;
        match DocError::from_response(409, body) {
            DocError::Conflict {
                server_revision,
                current,
            } => {
                assert_eq!(server_revision, 5);
                let cur = current.expect("current present");
                assert_eq!(cur.doc_id, "r1");
                assert_eq!(cur.revision, 5);
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn decodes_plain_api_error() {
        let body = r#"{"error":{"code":"forbidden","message":"nope"}}"#;
        match DocError::from_response(403, body) {
            DocError::Api { code, message, .. } => {
                assert_eq!(code, "forbidden");
                assert_eq!(message, "nope");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn profile_doc_roundtrips_without_game_path() {
        let doc = ProfileDoc {
            name: "Dice".into(),
            game_slug: "dice-drop".into(),
            game: Some("dice".into()),
            revision: None,
            front_url: Some("http://x".into()),
            resolutions: vec![],
            created_at: 42,
        };
        let json = serde_json::to_value(&doc).unwrap();
        assert!(json.get("gamePath").is_none());
        assert!(json.get("game_path").is_none());
        let back: ProfileDoc = serde_json::from_value(json.clone()).unwrap();
        // ResolutionPreset isn't PartialEq, so compare the serialized forms.
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    /// Live roundtrip against a running server. **Self-skips** unless
    /// `SDT_CLOUD_TEST_URL`, `SDT_CLOUD_TEST_TOKEN` (a `full`-scope Bearer PAT),
    /// and `SDT_CLOUD_TEST_SLUG` (a workspace the token can write) are all set —
    /// mirroring the server crate's `TEST_DATABASE_URL`-gated tests. Exercises
    /// create → get → stale-conflict → keep-mine LWW → since_seq list → delete
    /// through the real HTTP surface.
    #[tokio::test]
    async fn live_put_conflict_pull_roundtrip() {
        let (url, token, slug) = match (
            std::env::var("SDT_CLOUD_TEST_URL"),
            std::env::var("SDT_CLOUD_TEST_TOKEN"),
            std::env::var("SDT_CLOUD_TEST_SLUG"),
        ) {
            (Ok(u), Ok(t), Ok(s)) => (u, t, s),
            _ => {
                eprintln!(
                    "skipping live cloud test: set SDT_CLOUD_TEST_{{URL,TOKEN,SLUG}} to run it"
                );
                return;
            }
        };

        let conn = crate::cloud::http::Conn {
            http: reqwest::Client::new(),
            base: url.trim_end_matches('/').to_string(),
            token,
        };
        let docs = CloudDocuments { conn, slug };

        let doc_id = uuid::Uuid::new_v4().to_string();
        let data = serde_json::json!({
            "game_slug": "dice-drop", "mode": "base", "event_id": 1,
            "description": "live test", "created_at": 1u64
        });

        let created = docs
            .put(KIND_SAVED_ROUND, &doc_id, &data, None)
            .await
            .expect("create");
        let env = docs
            .get(KIND_SAVED_ROUND, &doc_id)
            .await
            .expect("get")
            .expect("present after create");
        assert_eq!(env.doc_id, doc_id);
        assert_eq!(env.revision, created.revision);

        // A stale base must conflict, surfacing the server's current revision.
        match docs
            .put(
                KIND_SAVED_ROUND,
                &doc_id,
                &data,
                Some(created.revision + 999),
            )
            .await
        {
            Err(DocError::Conflict {
                server_revision, ..
            }) => assert_eq!(server_revision, created.revision),
            other => panic!("expected conflict, got {other:?}"),
        }

        // Keep-mine LWW then resolves it against the fresh revision.
        put_lww(
            &docs,
            KIND_SAVED_ROUND,
            &doc_id,
            &data,
            Some(created.revision + 999),
        )
        .await
        .expect("keep-mine LWW resolves the conflict");

        let list = docs
            .list(Some(KIND_SAVED_ROUND), Some(0))
            .await
            .expect("since_seq list");
        assert!(list.documents.iter().any(|d| d.doc_id == doc_id));

        docs.delete(KIND_SAVED_ROUND, &doc_id, None)
            .await
            .expect("delete cleanup");
    }
}
