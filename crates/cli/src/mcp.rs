//! `sdt mcp` — a Model Context Protocol server over stdio.
//!
//! # Implementation choice
//!
//! This is a hand-rolled JSON-RPC 2.0 loop rather than the `rmcp` SDK. `rmcp`
//! 2.x builds a tool server from proc-macros (`#[tool_router]` / `#[tool]` /
//! `#[tool_handler]`) over a concrete `ServerHandler`; that does not compose
//! with a handler generic over our [`PlatformApi`] trait, which is exactly what
//! the offline dispatch test needs (drive the whole loop against a fake client,
//! no network). It would also inject macro-generated code under the crate's
//! `-D warnings` clippy gate. A plain tool server over stdio is a small, fully
//! specified surface — `initialize` / `notifications/initialized` / `tools/list`
//! / `tools/call` / `ping`, newline-delimited JSON — so hand-rolling keeps the
//! dependency footprint at zero new crates and the loop directly testable.
//!
//! The server URL and token come from the usual config precedence (handled by
//! the caller, which builds the [`ApiClient`]). `push_math` needs a token with
//! the `push:math` scope; the read tools need workspace membership only.

use std::path::Path;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::api::{ApiClient, ClientError, PlatformApi, resolve_head};
use crate::error::CliError;
use crate::output::Reporter;
use crate::{pull, push};

/// The protocol revision we advertise. We also accept the two prior revisions
/// on `initialize` and echo the client's when it is one we know.
const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const SERVER_NAME: &str = "sdt";

// ---------------------------------------------------------------------------
// Entry point + the generic stdio loop
// ---------------------------------------------------------------------------

/// Runs the MCP server on the process's stdin/stdout until EOF.
pub async fn run(client: ApiClient) -> Result<(), CliError> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    serve(stdin, stdout, &client)
        .await
        .map_err(|e| CliError::server(anyhow::anyhow!("mcp stdio error: {e}")))
}

/// The transport-agnostic loop: read newline-delimited JSON-RPC messages from
/// `reader`, dispatch each, and write a single compact JSON line per response.
/// Notifications (no `id`) produce no response. A single bad tool call never
/// stops the loop; it comes back as `isError` content.
async fn serve<R, W, A>(reader: R, mut writer: W, api: &A) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    A: PlatformApi,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(trimmed) {
            Ok(msg) => handle_message(api, msg).await,
            Err(e) => Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };
        if let Some(resp) = response {
            let bytes = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
            writer.write_all(&bytes).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

/// Dispatches one parsed JSON-RPC message, returning the response to write (or
/// `None` for a notification).
async fn handle_message<A: PlatformApi>(api: &A, msg: Value) -> Option<Value> {
    // A message with no `id` is a notification: never answer it.
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

    match method {
        "initialize" => id.map(|id| result_response(id, initialize_result(&msg))),
        "notifications/initialized" | "initialized" | "notifications/cancelled" => None,
        "ping" => id.map(|id| result_response(id, json!({}))),
        "tools/list" => id.map(|id| result_response(id, tools_list())),
        "tools/call" => {
            let id = id?;
            let result = call_tool(api, msg.get("params")).await;
            Some(result_response(id, result))
        }
        other => id.map(|id| error_response(id, -32601, &format!("method not found: {other}"))),
    }
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result(msg: &Value) -> Value {
    let requested = msg
        .get("params")
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str());
    let version = match requested {
        Some(v) if SUPPORTED_VERSIONS.contains(&v) => v,
        _ => PROTOCOL_VERSION,
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Stake Dev Tool platform access. Read tools cover workspaces, \
             games, revisions, and diffs; push_math commits a revision from a local math \
             folder (requires a push:math token); pull_revision downloads a revision to disk."
    })
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

/// Schema builder: an object with the given typed properties and required keys.
fn object_schema(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn tools_list() -> Value {
    let string = json!({ "type": "string" });
    let integer = json!({ "type": "integer" });
    json!({
        "tools": [
            tool(
                "list_workspaces",
                "List the workspaces the token can access.",
                object_schema(json!({}), &[]),
            ),
            tool(
                "list_games",
                "List a workspace's games with head revision and revision count.",
                object_schema(json!({ "workspace": string }), &["workspace"]),
            ),
            tool(
                "list_revisions",
                "List a game's revisions, newest first.",
                object_schema(
                    json!({ "workspace": string, "game": string, "limit": integer }),
                    &["workspace", "game"],
                ),
            ),
            tool(
                "get_revision",
                "Get a revision's detail including stats; omit number for the head.",
                object_schema(
                    json!({ "workspace": string, "game": string, "number": integer }),
                    &["workspace", "game"],
                ),
            ),
            tool(
                "diff_revisions",
                "Diff two revisions: file summary and per-mode stats deltas.",
                object_schema(
                    json!({
                        "workspace": string,
                        "game": string,
                        "after": integer,
                        "before": integer,
                    }),
                    &["workspace", "game", "after", "before"],
                ),
            ),
            tool(
                "push_math",
                "Push a local math folder as a new revision (needs a push:math token).",
                object_schema(
                    json!({
                        "workspace": string,
                        "game": string,
                        "path": string,
                        "message": string,
                        "parent_number": integer,
                    }),
                    &["workspace", "game", "path", "message"],
                ),
            ),
            tool(
                "pull_revision",
                "Download a revision's files to a directory; omit number for the head.",
                object_schema(
                    json!({
                        "workspace": string,
                        "game": string,
                        "number": integer,
                        "dest": string,
                    }),
                    &["workspace", "game", "dest"],
                ),
            ),
        ]
    })
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

/// Runs a `tools/call`, always returning a well-formed result object. Any error
/// (bad params, API failure, push/pull failure) becomes `isError` content —
/// the loop never crashes.
async fn call_tool<A: PlatformApi>(api: &A, params: Option<&Value>) -> Value {
    let name = params.and_then(|p| p.get("name")).and_then(|n| n.as_str());
    let empty = json!({});
    let args = params.and_then(|p| p.get("arguments")).unwrap_or(&empty);

    let Some(name) = name else {
        return tool_error_result(&ToolError::params("missing tool name in params"));
    };

    match dispatch_tool(api, name, args).await {
        Ok(text) => json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false,
        }),
        Err(e) => tool_error_result(&e),
    }
}

fn tool_error_result(e: &ToolError) -> Value {
    json!({
        "content": [{ "type": "text", "text": format!("{}: {}", e.code, e.message) }],
        "isError": true,
    })
}

async fn dispatch_tool<A: PlatformApi>(
    api: &A,
    name: &str,
    args: &Value,
) -> Result<String, ToolError> {
    match name {
        "list_workspaces" => Ok(api
            .list_workspaces()
            .await
            .map_err(ToolError::from_client)?
            .to_string()),
        "list_games" => {
            let ws = arg_str(args, "workspace")?;
            Ok(api
                .list_games(&ws)
                .await
                .map_err(ToolError::from_client)?
                .to_string())
        }
        "list_revisions" => {
            let ws = arg_str(args, "workspace")?;
            let game = arg_str(args, "game")?;
            let limit = arg_opt_u32(args, "limit")?;
            Ok(api
                .list_revisions(&ws, &game, limit)
                .await
                .map_err(ToolError::from_client)?
                .to_string())
        }
        "get_revision" => {
            let ws = arg_str(args, "workspace")?;
            let game = arg_str(args, "game")?;
            let number = resolve_number(api, &ws, &game, arg_opt_i64(args, "number")?).await?;
            let detail = api
                .get_revision(&ws, &game, number)
                .await
                .map_err(ToolError::from_client)?;
            serde_json::to_string(&detail).map_err(ToolError::encode)
        }
        "diff_revisions" => {
            let ws = arg_str(args, "workspace")?;
            let game = arg_str(args, "game")?;
            let after = arg_i64(args, "after")?;
            let before = arg_i64(args, "before")?;
            Ok(api
                .get_diff(&ws, &game, after, before)
                .await
                .map_err(ToolError::from_client)?
                .to_string())
        }
        "push_math" => {
            let ws = arg_str(args, "workspace")?;
            let game = arg_str(args, "game")?;
            let path = arg_str(args, "path")?;
            let message = arg_str(args, "message")?;
            let parent = arg_opt_i64(args, "parent_number")?;
            let reporter = Reporter::quiet();
            let outcome = push::push_folder(
                api,
                Path::new(&path),
                &ws,
                &game,
                &message,
                parent,
                &reporter,
            )
            .await
            .map_err(ToolError::from_cli)?;
            Ok(json!({
                "number": outcome.detail.number,
                "uploaded": outcome.uploaded_count,
                "deduplicated_bytes": outcome.total_bytes.saturating_sub(outcome.uploaded_bytes),
            })
            .to_string())
        }
        "pull_revision" => {
            let ws = arg_str(args, "workspace")?;
            let game = arg_str(args, "game")?;
            let dest = arg_str(args, "dest")?;
            let number = resolve_number(api, &ws, &game, arg_opt_i64(args, "number")?).await?;
            let reporter = Reporter::quiet();
            // force = true: an agent asked to pull into `dest` expects it to just
            // happen, even on a re-run into the same directory.
            let files =
                pull::pull_files(api, &ws, &game, number, Path::new(&dest), &reporter, true)
                    .await
                    .map_err(ToolError::from_cli)?;
            Ok(json!({ "number": number, "dest": dest, "files": files }).to_string())
        }
        other => Err(ToolError {
            code: "unknown_tool".to_string(),
            message: format!("no such tool: {other}"),
        }),
    }
}

/// Resolves a possibly-omitted revision number to a concrete one (head).
async fn resolve_number<A: PlatformApi>(
    api: &A,
    ws: &str,
    game: &str,
    number: Option<i64>,
) -> Result<i64, ToolError> {
    match number {
        Some(n) => Ok(n),
        None => resolve_head(api, ws, game)
            .await
            .map_err(ToolError::from_client),
    }
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ToolError::params(format!("missing or non-string argument: {key}")))
}

fn arg_i64(args: &Value, key: &str) -> Result<i64, ToolError> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| ToolError::params(format!("missing or non-integer argument: {key}")))
}

fn arg_opt_i64(args: &Value, key: &str) -> Result<Option<i64>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| ToolError::params(format!("argument {key} must be an integer"))),
    }
}

fn arg_opt_u32(args: &Value, key: &str) -> Result<Option<u32>, ToolError> {
    match arg_opt_i64(args, key)? {
        None => Ok(None),
        Some(n) if (0..=i64::from(u32::MAX)).contains(&n) => Ok(Some(n as u32)),
        Some(_) => Err(ToolError::params(format!("argument {key} is out of range"))),
    }
}

// ---------------------------------------------------------------------------
// Tool error type
// ---------------------------------------------------------------------------

/// A tool-call failure, rendered into `isError` content as `code: message`.
struct ToolError {
    code: String,
    message: String,
}

impl ToolError {
    fn params(msg: impl Into<String>) -> Self {
        Self {
            code: "invalid_params".to_string(),
            message: msg.into(),
        }
    }

    /// Preserves the server's stable error `code` (e.g. `revision_not_found`).
    fn from_client(e: ClientError) -> Self {
        match e {
            ClientError::Api(a) => Self {
                code: a.code,
                message: a.message,
            },
            ClientError::Transport(m) => Self {
                code: "network".to_string(),
                message: m,
            },
            ClientError::Other(m) => Self {
                code: "error".to_string(),
                message: m,
            },
        }
    }

    fn from_cli(e: CliError) -> Self {
        Self {
            code: e.code().to_string(),
            message: e.message(),
        }
    }

    fn encode(e: serde_json::Error) -> Self {
        Self {
            code: "encode_error".to_string(),
            message: e.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: drive the stdio loop in-process against a fake PlatformApi.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{
        BlobUpload, ClientResult, CreateRevisionRequest, FileDownload, FileEntry, RevisionApi,
        RevisionDetail,
    };
    use crate::output::FileProgress;

    /// A fake platform: canned reads, no network, no disk.
    struct FakePlatform;

    fn fake_detail(number: i64) -> RevisionDetail {
        RevisionDetail {
            number,
            message: Some("fake".into()),
            created_at: None,
            files: vec![],
            stats: None,
            extra: Default::default(),
        }
    }

    impl RevisionApi for FakePlatform {
        async fn check_files(
            &self,
            _ws: &str,
            _game: &str,
            _files: &[FileEntry],
        ) -> ClientResult<Vec<String>> {
            Ok(vec![])
        }
        async fn upload_blob(
            &self,
            _ws: &str,
            _game: &str,
            _upload: &BlobUpload,
            _progress: FileProgress,
        ) -> ClientResult<()> {
            Ok(())
        }
        async fn create_revision(
            &self,
            _ws: &str,
            _game: &str,
            _req: &CreateRevisionRequest,
        ) -> ClientResult<RevisionDetail> {
            Ok(fake_detail(1))
        }
        async fn get_revision(
            &self,
            _ws: &str,
            _game: &str,
            number: i64,
        ) -> ClientResult<RevisionDetail> {
            Ok(fake_detail(number))
        }
    }

    impl PlatformApi for FakePlatform {
        async fn list_workspaces(&self) -> ClientResult<Value> {
            Ok(json!({ "workspaces": [{ "slug": "acme", "name": "Acme", "role": "owner" }] }))
        }
        async fn list_games(&self, _ws: &str) -> ClientResult<Value> {
            Ok(json!({ "games": [] }))
        }
        async fn list_revisions(
            &self,
            _ws: &str,
            _game: &str,
            _limit: Option<u32>,
        ) -> ClientResult<Value> {
            Ok(json!({ "revisions": [{ "number": 7 }] }))
        }
        async fn get_diff(
            &self,
            _ws: &str,
            _game: &str,
            _after: i64,
            _before: i64,
        ) -> ClientResult<Value> {
            Ok(json!({ "files": { "unchanged": 0 }, "stats": { "modes": [] } }))
        }
        async fn download_file(
            &self,
            _spec: &FileDownload<'_>,
            _progress: FileProgress,
        ) -> ClientResult<()> {
            Ok(())
        }
    }

    /// Runs `input` (already newline-delimited) through the loop, returning the
    /// parsed response lines.
    async fn drive(input: &str) -> Vec<Value> {
        let mut output: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut output, &FakePlatform)
            .await
            .expect("serve loop");
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).expect("valid json response"))
            .collect()
    }

    #[tokio::test]
    async fn handshake_list_and_call() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_workspaces","arguments":{}}}"#,
            "\n",
        );
        let responses = drive(input).await;
        // initialize + tools/list + tools/call → 3 responses (notification is silent).
        assert_eq!(responses.len(), 3);

        // initialize
        assert_eq!(responses[0]["id"], json!(1));
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "sdt");
        assert!(responses[0]["result"]["capabilities"]["tools"].is_object());

        // tools/list
        let tools = responses[1]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 7);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for expected in [
            "list_workspaces",
            "list_games",
            "list_revisions",
            "get_revision",
            "diff_revisions",
            "push_math",
            "pull_revision",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }

        // tools/call list_workspaces → isError false, JSON text with our fake ws.
        assert_eq!(responses[2]["result"]["isError"], json!(false));
        let text = responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["workspaces"][0]["slug"], "acme");
    }

    #[tokio::test]
    async fn get_revision_defaults_to_head() {
        // number omitted → resolve_head via list_revisions → 7 → get_revision(7).
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_revision","arguments":{"workspace":"acme","game":"g"}}}"#,
            "\n",
        );
        let responses = drive(input).await;
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["result"]["isError"], json!(false));
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["number"], json!(7));
    }

    #[tokio::test]
    async fn unknown_tool_is_iserror_not_a_crash() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
            "\n",
        );
        let responses = drive(input).await;
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["result"]["isError"], json!(true));
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("unknown_tool"));
    }

    #[tokio::test]
    async fn missing_argument_reports_iserror() {
        // list_games without the required workspace arg.
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_games","arguments":{}}}"#,
            "\n",
        );
        let responses = drive(input).await;
        assert_eq!(responses[0]["result"]["isError"], json!(true));
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("invalid_params"));
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":9,"method":"resources/list"}"#,
            "\n",
        );
        let responses = drive(input).await;
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["error"]["code"], json!(-32601));
    }

    #[tokio::test]
    async fn ping_and_notifications() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
        );
        let responses = drive(input).await;
        // ping answered; notification silent.
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], json!(1));
        assert!(responses[0]["result"].is_object());
    }
}
