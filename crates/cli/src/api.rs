//! The single place every wire shape and HTTP call lives, so a later
//! reconciliation against the real `crates/server`/`crates/protocol` types has
//! exactly one file to touch.
//!
//! Push orchestration is written against the [`RevisionApi`] trait rather than
//! [`ApiClient`] directly, so it can be unit tested with a fake client and no
//! network.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::hash;
use crate::output::FileProgress;

/// Read size for streaming a file into an upload body.
const UPLOAD_CHUNK: usize = 64 * 1024;

pub type ClientResult<T> = Result<T, ClientError>;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A structured error decoded from a non-2xx JSON response. `code` is the
/// stable machine string the orchestrator branches on (`missing_blobs`,
/// `stale_parent`, the device-flow codes, …).
#[derive(Debug, Clone, Error)]
#[error("{message} [{code}] (HTTP {status})")]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    /// Blob hashes the server is still missing (409 `missing_blobs` only).
    pub missing: Vec<String>,
}

/// Anything that can go wrong talking to the server.
#[derive(Debug, Error)]
pub enum ClientError {
    /// The server answered with a structured error envelope.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// Transport failure: DNS, connection refused, TLS, timeout, socket.
    #[error("network error: {0}")]
    Transport(String),
    /// A 2xx body that could not be decoded, or a similar local surprise.
    #[error("{0}")]
    Other(String),
}

impl ClientError {
    /// Whether retrying the same request could plausibly succeed. Transport
    /// blips and 5xx/429 are retryable; 4xx (bad request, auth) are not.
    pub fn is_retryable(&self) -> bool {
        match self {
            ClientError::Transport(_) => true,
            ClientError::Api(e) => e.status >= 500 || e.status == 429,
            ClientError::Other(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// One manifest entry: path, content hash, size. Used in both the check and
/// commit request bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

#[derive(Serialize)]
struct CheckRequest<'a> {
    files: &'a [FileEntry],
}

#[derive(Deserialize)]
struct CheckResponse {
    #[serde(default)]
    missing: Vec<String>,
}

/// Body of `POST …/revisions`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateRevisionRequest {
    pub message: String,
    pub files: Vec<FileEntry>,
    /// `None` serializes to `null`, which the server reads as "auto-parent".
    pub parent_number: Option<i64>,
}

/// A revision as returned by create and detail. Unknown fields are preserved in
/// `extra` so `--json` stays faithful as the server grows the shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionDetail {
    pub number: i64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub stats: Option<Stats>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Per-revision bet-stats block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    /// `"pending" | "ok" | "error"`.
    pub status: String,
    #[serde(default)]
    pub modes: Vec<ModeStat>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One mode's computed stats. `cost` deserializes from an integer or a float.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeStat {
    pub mode: String,
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub rtp: Option<f64>,
    #[serde(default)]
    pub max_win: Option<f64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One row of the revision list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionSummary {
    pub number: i64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub author_display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub files_count: Option<i64>,
    #[serde(default)]
    pub total_size: Option<i64>,
    #[serde(default)]
    pub stats_status: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionList {
    #[serde(default)]
    pub revisions: Vec<RevisionSummary>,
}

/// One workspace row from `GET /api/workspaces`. Lenient: unknown fields (id,
/// created_at, …) are preserved in `extra` so `--json` stays faithful.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub slug: String,
    #[serde(default)]
    pub name: Option<String>,
    /// `"owner" | "admin" | "member"`.
    #[serde(default)]
    pub role: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspacesResponse {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
}

/// One game row from `GET …/games`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameInfo {
    pub slug: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Highest revision number, or `null` when the game has no revisions yet.
    #[serde(default)]
    pub head_number: Option<i64>,
    #[serde(default)]
    pub revisions_count: Option<i64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamesResponse {
    #[serde(default)]
    pub games: Vec<GameInfo>,
}

/// A file present in both revisions but with different content (diff).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    #[serde(default)]
    pub before_size: Option<i64>,
    #[serde(default)]
    pub after_size: Option<i64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// File-level diff: `before` = the `:other` revision, `after` = the `:number`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileDiff {
    #[serde(default)]
    pub added: Vec<FileEntry>,
    #[serde(default)]
    pub removed: Vec<FileEntry>,
    #[serde(default)]
    pub changed: Vec<ChangedFile>,
    #[serde(default)]
    pub unchanged: u32,
}

/// Before/after stats for a single mode. Either side is `null` when its
/// revision's stats aren't `ok`, or when the mode is absent on that side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeStatDiff {
    pub mode: String,
    #[serde(default)]
    pub before: Option<ModeStat>,
    #[serde(default)]
    pub after: Option<ModeStat>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatsDiff {
    #[serde(default)]
    pub modes: Vec<ModeStatDiff>,
}

/// `GET …/revisions/:number/diff/:other` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionDiff {
    #[serde(default)]
    pub files: FileDiff,
    #[serde(default)]
    pub stats: StatsDiff,
}

/// Everything a single file pull needs, bundled to keep the download signature
/// under clippy's argument-count limit.
pub struct FileDownload<'a> {
    pub ws: &'a str,
    pub game: &'a str,
    pub number: i64,
    /// Forward-slashed path inside the revision (the `*path` route segment).
    pub remote_path: &'a str,
    /// Where to write the bytes on disk.
    pub dest: PathBuf,
    /// The lowercase-hex sha256 the downloaded bytes must match.
    pub expected_hash: &'a str,
}

/// Response to `POST …/device/code`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub expires_in: i64,
    #[serde(default)]
    pub interval: i64,
}

/// The success body of `POST …/device/token`. Extra fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceTokenSuccess {
    pub token: String,
}

/// A blob queued for upload, carrying everything the streaming PUT needs.
#[derive(Debug, Clone)]
pub struct BlobUpload {
    pub hash: String,
    pub path: PathBuf,
    pub size: u64,
    /// Display name (relative path) for progress lines.
    pub rel_path: String,
}

// ---------------------------------------------------------------------------
// The trait push orchestration is written against
// ---------------------------------------------------------------------------

/// The revision operations `push` needs. Kept as a trait (implemented by
/// [`ApiClient`] for real, by a fake in tests) so orchestration logic — dedup,
/// the missing_blobs retry, the stale_parent abort — is testable offline.
pub trait RevisionApi {
    fn check_files(
        &self,
        ws: &str,
        game: &str,
        files: &[FileEntry],
    ) -> impl std::future::Future<Output = ClientResult<Vec<String>>>;

    fn upload_blob(
        &self,
        ws: &str,
        game: &str,
        upload: &BlobUpload,
        progress: FileProgress,
    ) -> impl std::future::Future<Output = ClientResult<()>>;

    fn create_revision(
        &self,
        ws: &str,
        game: &str,
        req: &CreateRevisionRequest,
    ) -> impl std::future::Future<Output = ClientResult<RevisionDetail>>;

    fn get_revision(
        &self,
        ws: &str,
        game: &str,
        number: i64,
    ) -> impl std::future::Future<Output = ClientResult<RevisionDetail>>;
}

/// The full platform surface the `mcp` server and the read commands drive.
/// A supertrait of [`RevisionApi`] so `push` orchestration keeps working
/// through it, while the extra read/download operations let the whole MCP tool
/// dispatch be exercised offline against a single fake implementation.
pub trait PlatformApi: RevisionApi {
    /// `GET /api/workspaces` — raw JSON, echoed faithfully.
    fn list_workspaces(&self)
    -> impl std::future::Future<Output = ClientResult<serde_json::Value>>;

    /// `GET …/games` — raw JSON.
    fn list_games(
        &self,
        ws: &str,
    ) -> impl std::future::Future<Output = ClientResult<serde_json::Value>>;

    /// `GET …/revisions` (newest first) — raw JSON.
    fn list_revisions(
        &self,
        ws: &str,
        game: &str,
        limit: Option<u32>,
    ) -> impl std::future::Future<Output = ClientResult<serde_json::Value>>;

    /// `GET …/revisions/:after/diff/:before` — raw JSON.
    fn get_diff(
        &self,
        ws: &str,
        game: &str,
        after: i64,
        before: i64,
    ) -> impl std::future::Future<Output = ClientResult<serde_json::Value>>;

    /// `GET …/revisions/:number/files/*path` — stream to `spec.dest`, verifying
    /// the sha256 while writing and advancing `progress`.
    fn download_file(
        &self,
        spec: &FileDownload<'_>,
        progress: FileProgress,
    ) -> impl std::future::Future<Output = ClientResult<()>>;
}

/// Resolves a game's head (highest) revision number from the revisions list.
pub async fn resolve_head<C: PlatformApi>(client: &C, ws: &str, game: &str) -> ClientResult<i64> {
    let value = client.list_revisions(ws, game, Some(1)).await?;
    let list: RevisionList = serde_json::from_value(value)
        .map_err(|e| ClientError::Other(format!("could not parse revisions: {e}")))?;
    list.revisions
        .first()
        .map(|r| r.number)
        .ok_or_else(|| ClientError::Other(format!("game '{game}' has no revisions yet")))
}

// ---------------------------------------------------------------------------
// The real client
// ---------------------------------------------------------------------------

/// A thin HTTP client bound to a base URL and (optionally) a bearer token.
pub struct ApiClient {
    http: Client,
    base: String,
    token: Option<String>,
}

impl ApiClient {
    pub fn new(base: &str, token: Option<String>) -> anyhow::Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("sdt/", env!("CARGO_PKG_VERSION")))
            // Bound connection setup, but never the request as a whole — a big
            // book upload can legitimately run for minutes.
            .connect_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn authed(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    /// Requests a device code to start the login flow (no auth).
    pub async fn device_code(&self) -> ClientResult<DeviceCodeResponse> {
        let resp = self
            .http
            .post(self.url("/api/auth/device/code"))
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }

    /// Polls for the token once. A pending/denied/expired state comes back as
    /// [`ClientError::Api`] with the corresponding `code`.
    pub async fn device_token(&self, device_code: &str) -> ClientResult<DeviceTokenSuccess> {
        let resp = self
            .http
            .post(self.url("/api/auth/device/token"))
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }

    /// Lists revisions, returning the raw JSON so `--json` can echo it exactly.
    pub async fn list_revisions_raw(
        &self,
        ws: &str,
        game: &str,
        limit: Option<u32>,
    ) -> ClientResult<serde_json::Value> {
        let mut req = self.authed(
            self.http
                .get(self.url(&format!("/api/workspaces/{ws}/games/{game}/revisions"))),
        );
        if let Some(n) = limit {
            req = req.query(&[("limit", n.to_string())]);
        }
        let resp = req.send().await.map_err(map_transport)?;
        read_body_typed(resp).await
    }

    /// `GET /api/auth/me` — the current user, as raw JSON.
    pub async fn get_me(&self) -> ClientResult<serde_json::Value> {
        let resp = self
            .authed(self.http.get(self.url("/api/auth/me")))
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }
}

impl RevisionApi for ApiClient {
    async fn check_files(
        &self,
        ws: &str,
        game: &str,
        files: &[FileEntry],
    ) -> ClientResult<Vec<String>> {
        let resp = self
            .authed(self.http.post(self.url(&format!(
                "/api/workspaces/{ws}/games/{game}/revisions/check"
            ))))
            .json(&CheckRequest { files })
            .send()
            .await
            .map_err(map_transport)?;
        let out: CheckResponse = read_body_typed(resp).await?;
        Ok(out.missing)
    }

    async fn upload_blob(
        &self,
        ws: &str,
        game: &str,
        upload: &BlobUpload,
        progress: FileProgress,
    ) -> ClientResult<()> {
        // Stream straight from disk: the file is never read whole into memory,
        // and each chunk advances the progress bar as it goes on the wire.
        let file = tokio::fs::File::open(&upload.path)
            .await
            .map_err(|e| ClientError::Transport(format!("open {}: {e}", upload.path.display())))?;
        let stream = ReaderStream::with_capacity(file, UPLOAD_CHUNK).map(move |chunk| {
            if let Ok(bytes) = &chunk {
                progress.inc(bytes.len() as u64);
            }
            chunk
        });
        let body = reqwest::Body::wrap_stream(stream);
        let resp = self
            .authed(self.http.put(self.url(&format!(
                "/api/workspaces/{ws}/games/{game}/blobs/{}",
                upload.hash
            ))))
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(map_transport)?;
        expect_success(resp).await
    }

    async fn create_revision(
        &self,
        ws: &str,
        game: &str,
        req: &CreateRevisionRequest,
    ) -> ClientResult<RevisionDetail> {
        let resp = self
            .authed(
                self.http
                    .post(self.url(&format!("/api/workspaces/{ws}/games/{game}/revisions"))),
            )
            .json(req)
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }

    async fn get_revision(
        &self,
        ws: &str,
        game: &str,
        number: i64,
    ) -> ClientResult<RevisionDetail> {
        let resp = self
            .authed(self.http.get(self.url(&format!(
                "/api/workspaces/{ws}/games/{game}/revisions/{number}"
            ))))
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }
}

impl PlatformApi for ApiClient {
    async fn list_workspaces(&self) -> ClientResult<serde_json::Value> {
        let resp = self
            .authed(self.http.get(self.url("/api/workspaces")))
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }

    async fn list_games(&self, ws: &str) -> ClientResult<serde_json::Value> {
        let resp = self
            .authed(
                self.http
                    .get(self.url(&format!("/api/workspaces/{ws}/games"))),
            )
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }

    async fn list_revisions(
        &self,
        ws: &str,
        game: &str,
        limit: Option<u32>,
    ) -> ClientResult<serde_json::Value> {
        // Delegate to the inherent raw method the `revisions` command uses.
        self.list_revisions_raw(ws, game, limit).await
    }

    async fn get_diff(
        &self,
        ws: &str,
        game: &str,
        after: i64,
        before: i64,
    ) -> ClientResult<serde_json::Value> {
        let resp = self
            .authed(self.http.get(self.url(&format!(
                "/api/workspaces/{ws}/games/{game}/revisions/{after}/diff/{before}"
            ))))
            .send()
            .await
            .map_err(map_transport)?;
        read_body_typed(resp).await
    }

    async fn download_file(
        &self,
        spec: &FileDownload<'_>,
        progress: FileProgress,
    ) -> ClientResult<()> {
        let resp = self
            .authed(self.http.get(self.url(&format!(
                "/api/workspaces/{}/games/{}/revisions/{}/files/{}",
                spec.ws, spec.game, spec.number, spec.remote_path
            ))))
            .send()
            .await
            .map_err(map_transport)?;
        // Surface a structured error before touching the disk.
        let status = resp.status();
        if !status.is_success() {
            let bytes = resp.bytes().await.map_err(map_transport)?;
            return Err(ClientError::Api(parse_api_error(status.as_u16(), &bytes)));
        }

        let mut file = tokio::fs::File::create(&spec.dest)
            .await
            .map_err(|e| ClientError::Other(format!("create {}: {e}", spec.dest.display())))?;
        let mut hasher = Sha256::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_transport)?;
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| ClientError::Other(format!("write {}: {e}", spec.dest.display())))?;
            progress.inc(chunk.len() as u64);
        }
        file.flush()
            .await
            .map_err(|e| ClientError::Other(format!("flush {}: {e}", spec.dest.display())))?;

        let got = hash::to_hex(&hasher.finalize());
        if got != spec.expected_hash {
            // Remove the corrupt file so a retry starts clean.
            let _ = tokio::fs::remove_file(&spec.dest).await;
            return Err(ClientError::Other(format!(
                "sha256 mismatch for {}: expected {}, got {got}",
                spec.remote_path, spec.expected_hash
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

fn map_transport(e: reqwest::Error) -> ClientError {
    ClientError::Transport(e.to_string())
}

/// Reads a JSON body: parses `T` on 2xx, else decodes the error envelope.
async fn read_body_typed<T: DeserializeOwned>(resp: Response) -> ClientResult<T> {
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(map_transport)?;
    if status.is_success() {
        serde_json::from_slice::<T>(&bytes)
            .map_err(|e| ClientError::Other(format!("could not decode response: {e}")))
    } else {
        Err(ClientError::Api(parse_api_error(status.as_u16(), &bytes)))
    }
}

/// Checks a response that carries no body of interest (e.g. a blob PUT).
async fn expect_success(resp: Response) -> ClientResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let bytes = resp.bytes().await.map_err(map_transport)?;
    Err(ClientError::Api(parse_api_error(status.as_u16(), &bytes)))
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    #[serde(default)]
    error: ErrorDetail,
    // The 409 missing_blobs list may sit at the top level…
    #[serde(default)]
    missing: Vec<String>,
}

#[derive(Deserialize, Default)]
struct ErrorDetail {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    // …or nested under `error`; accept either.
    #[serde(default)]
    missing: Vec<String>,
}

/// Decodes the `{"error":{code,message},"missing":[…]}` envelope, falling back
/// to a synthetic error when the body isn't the expected JSON.
fn parse_api_error(status: u16, bytes: &[u8]) -> ApiError {
    if let Ok(env) = serde_json::from_slice::<ErrorEnvelope>(bytes) {
        let has_content = !env.error.code.is_empty()
            || !env.error.message.is_empty()
            || !env.missing.is_empty()
            || !env.error.missing.is_empty();
        if has_content {
            let missing = if env.missing.is_empty() {
                env.error.missing
            } else {
                env.missing
            };
            return ApiError {
                status,
                code: if env.error.code.is_empty() {
                    format!("http_{status}")
                } else {
                    env.error.code
                },
                message: if env.error.message.is_empty() {
                    format!("HTTP {status}")
                } else {
                    env.error.message
                },
                missing,
            };
        }
    }
    let text = String::from_utf8_lossy(bytes);
    let snippet: String = text.trim().chars().take(300).collect();
    ApiError {
        status,
        code: format!("http_{status}"),
        message: if snippet.is_empty() {
            format!("HTTP {status}")
        } else {
            snippet
        },
        missing: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_missing_blobs_top_level() {
        let body =
            br#"{"error":{"code":"missing_blobs","message":"upload first"},"missing":["aa","bb"]}"#;
        let err = parse_api_error(409, body);
        assert_eq!(err.code, "missing_blobs");
        assert_eq!(err.missing, ["aa", "bb"]);
        assert_eq!(err.status, 409);
    }

    #[test]
    fn parses_missing_blobs_nested_under_error() {
        let body = br#"{"error":{"code":"missing_blobs","message":"x","missing":["cc"]}}"#;
        let err = parse_api_error(409, body);
        assert_eq!(err.code, "missing_blobs");
        assert_eq!(err.missing, ["cc"]);
    }

    #[test]
    fn parses_plain_error_envelope() {
        let body = br#"{"error":{"code":"stale_parent","message":"rev moved on"}}"#;
        let err = parse_api_error(409, body);
        assert_eq!(err.code, "stale_parent");
        assert!(err.missing.is_empty());
        assert_eq!(err.message, "rev moved on");
    }

    #[test]
    fn falls_back_on_non_json_body() {
        let err = parse_api_error(502, b"<html>bad gateway</html>");
        assert_eq!(err.code, "http_502");
        assert_eq!(err.status, 502);
    }

    #[test]
    fn retryable_classification() {
        assert!(ClientError::Transport("x".into()).is_retryable());
        assert!(
            ClientError::Api(ApiError {
                status: 503,
                code: "x".into(),
                message: "y".into(),
                missing: vec![]
            })
            .is_retryable()
        );
        assert!(
            !ClientError::Api(ApiError {
                status: 400,
                code: "x".into(),
                message: "y".into(),
                missing: vec![]
            })
            .is_retryable()
        );
    }
}
