# server

Axum server for the Stake Dev Tool cloud platform (V2). Single static binary:
self-host is this binary (or its Docker image) + Postgres + an object store.

## Prerequisites

- Rust (workspace toolchain) and `cargo`
- Docker + Docker Compose (for local Postgres and MinIO)

## Run it

Start the local backing services (Postgres on `5433`, MinIO on `9000`/`9001`):

```sh
docker compose -f docker-compose.dev.yml up -d
```

Copy the example environment and run the server:

```sh
cp .env.example .env
cargo run -p server
```

The server binds `127.0.0.1:8080` by default. It starts immediately and serves
`/healthz` even before Postgres is reachable; migrations run in the background
with retry/backoff and readiness is reflected in the probe.

```sh
curl -i http://127.0.0.1:8080/healthz
```

`200` with `"status":"ok"` once the database and object store both answer;
`503` with `"status":"degraded"` while either is down (the JSON names the
failing component).

## Configuration

All configuration is via environment variables (loaded from `.env` if present).

| Variable | Default | Description |
| --- | --- | --- |
| `SERVER_BIND_ADDR` | `127.0.0.1:8080` | Address the HTTP server binds. |
| `DATABASE_URL` | `postgres://stakedev:stakedev@localhost:5433/stakedev` | Postgres connection string. |
| `STORAGE_BACKEND` | `fs` | `fs` (local directory) or `s3` (S3-compatible). |
| `STORAGE_FS_ROOT` | `./data/blobs` | Root directory for the `fs` backend. |
| `STORAGE_S3_ENDPOINT` | _(unset)_ | Custom endpoint for MinIO/R2 (`s3` backend). |
| `STORAGE_S3_BUCKET` | _(required for `s3`)_ | Bucket name. |
| `STORAGE_S3_REGION` | `auto` | Region (`auto` works for R2). |
| `STORAGE_S3_ACCESS_KEY_ID` | _(unset)_ | Access key id. |
| `STORAGE_S3_SECRET_ACCESS_KEY` | _(unset)_ | Secret access key. |
| `STORAGE_S3_ALLOW_HTTP` | `false` | Allow plaintext HTTP (needed for local MinIO). |
| `STORAGE_MAX_BLOB_BYTES` | `8589934592` (8 GiB) | Max bytes per blob upload; larger bodies are aborted with `413`. |
| `SERVER_COOKIE_SECURE` | `false` | `Secure` flag on the session cookie; set `true` behind TLS. |
| `SERVER_PUBLIC_URL` | _(unset)_ | Public base URL for invite/device/OAuth links; falls back to the bind address. |
| `GITHUB_CLIENT_ID` | _(unset)_ | GitHub OAuth app client id (enables GitHub sign-in with the two below). |
| `GITHUB_CLIENT_SECRET` | _(unset)_ | GitHub OAuth app client secret. |
| `RUST_LOG` | `info` | `tracing` env-filter directive. |
| `TEST_DATABASE_URL` | _(unset)_ | Enables the real-database integration tests when set. |

GitHub OAuth is active only when `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`,
and `SERVER_PUBLIC_URL` are all set; otherwise the `/api/auth/github/*` routes
return `404` and `GET /api/auth/providers` reports `"github": false`.

## API

All application endpoints live under `/api` and return a uniform error envelope
on failure: `{"error": {"code": "...", "message": "..."}}`. Authentication is a
session cookie (`sdt_session`) **or** an `Authorization: Bearer sdt_pat_…`
personal API token; endpoints marked _session_ reject API tokens (a token cannot
mint tokens).

| Method | Path | Auth | Purpose |
| --- | --- | --- | --- |
| `POST` | `/api/auth/register` | — | Create a password account; sets a session cookie. |
| `POST` | `/api/auth/login` | — | Password login (rate-limited); sets a session cookie. |
| `POST` | `/api/auth/logout` | cookie | Delete the session and clear the cookie. |
| `GET` | `/api/auth/me` | user | The current user. |
| `GET` | `/api/auth/providers` | — | Capability flags (`password`, `github`). |
| `GET` | `/api/auth/github/start` | — | Begin GitHub OAuth (404 if unconfigured). |
| `GET` | `/api/auth/github/callback` | — | GitHub OAuth callback (404 if unconfigured). |
| `POST` | `/api/auth/device/code` | — | Start device pairing; returns a device + user code. |
| `POST` | `/api/auth/device/token` | — | Poll for the device token (RFC 8628 error shape). |
| `POST` | `/api/auth/device/approve` | session | Approve/deny a device by its user code. |
| `GET` | `/api/tokens` | session | List the caller's API tokens. |
| `POST` | `/api/tokens` | session | Mint an API token (secret shown once). |
| `DELETE` | `/api/tokens/:id` | session | Revoke an API token. |
| `POST` | `/api/workspaces` | user | Create a workspace (caller becomes owner). |
| `GET` | `/api/workspaces` | user | List the caller's workspaces with roles. |
| `GET` | `/api/workspaces/:slug` | user | Workspace detail + members (members only). |
| `PATCH` | `/api/workspaces/:slug/members/:user_id` | user | Change a member's role (owner/admin). |
| `DELETE` | `/api/workspaces/:slug/members/:user_id` | user | Remove a member / leave. |
| `POST` | `/api/workspaces/:slug/invites` | user | Create an invite (owner/admin). |
| `GET` | `/api/workspaces/:slug/invites` | user | List invites (owner/admin). |
| `DELETE` | `/api/workspaces/:slug/invites/:id` | user | Revoke an invite (owner/admin). |
| `GET` | `/api/invites/:token` | — | Public invite preview for the accept page. |
| `POST` | `/api/invites/:token/accept` | session | Accept an invite (grants membership). |

### Math revisions (M2)

Content-addressed math blobs with per-workspace dedup, immutable numbered
revisions, file diffs, and per-revision bet stats. Writes need workspace
membership **and** the `push:math` scope (a session's implicit `full` scope
satisfies it); reads need membership only. Hashes are lowercase hex sha256.

| Method | Path | Auth | Purpose |
| --- | --- | --- | --- |
| `GET` | `/api/workspaces/:slug/games` | member | List games with `head_number` + `revisions_count`. |
| `POST` | `/api/workspaces/:slug/games/:game/revisions/check` | push:math | Validate a manifest; return the `missing` blob hashes. |
| `PUT` | `/api/workspaces/:slug/games/:game/blobs/:hash` | push:math | Stream-upload a blob (201 new, 200 exists, 422 hash mismatch, 413 too large). |
| `GET` | `/api/workspaces/:slug/games/:game/blobs/:hash` | member | Stream a blob's bytes. |
| `POST` | `/api/workspaces/:slug/games/:game/revisions` | push:math | Commit a revision (409 `missing_blobs` / `stale_parent`). |
| `GET` | `/api/workspaces/:slug/games/:game/revisions` | member | List revisions (newest first). |
| `GET` | `/api/workspaces/:slug/games/:game/revisions/:number` | member | Revision detail: manifest + stats. |
| `GET` | `/api/workspaces/:slug/games/:game/revisions/:number/diff/:other` | member | File + stats diff (`:other` = before, `:number` = after). |
| `GET` | `/api/workspaces/:slug/games/:game/revisions/:number/files/*path` | member | Stream a file's blob (pull). |

### Document sync + workspace SSE (M3)

Server-authoritative versioned JSON documents (`profile`, `saved_round`) with
optimistic concurrency: a `PUT`/`DELETE` carries `base_revision` and applies only
when it equals the document's current `revision`, otherwise `409 document_conflict`
with the current server envelope in the body (`{ error, current }`) so the client
can reconcile. Every write bumps the per-document `revision`, advances the
workspace-global `seq` cursor, and nudges SSE subscribers with a `document` event.
Reads need membership; writes need the `full` scope (a `push:math`-only PAT can
read documents but not edit them). Payloads are validated per kind (required
fields enforced, unknown fields tolerated and preserved) and capped at 64 KiB.

`DocumentEnvelope = { kind, doc_id, data, revision, seq, updated_by_display,
updated_at, deleted }`. A list includes tombstones only when `since_seq` is given
(a sync pull); a plain list omits them.

| Method | Path | Auth | Purpose |
| --- | --- | --- | --- |
| `GET` | `/api/workspaces/:slug/documents?kind=&since_seq=` | member | List documents `{ documents, latest_seq }`; `since_seq` pulls newer rows incl. tombstones. |
| `GET` | `/api/workspaces/:slug/documents/:kind/:doc_id` | member | One live document envelope (404 if tombstoned/missing). |
| `PUT` | `/api/workspaces/:slug/documents/:kind/:doc_id` | full | Create/update `{ data, base_revision }` → `{ revision, seq }` (409 `document_conflict`). |
| `DELETE` | `/api/workspaces/:slug/documents/:kind/:doc_id` | full | Tombstone `{ base_revision }` → `{ seq }` (409 `document_conflict`). |
| `GET` | `/api/workspaces/:slug/events` | member | Workspace SSE stream (cookie or Bearer). |
| `DELETE` | `/api/workspaces/:slug` | owner | Delete a workspace (204); blob bytes swept asynchronously. |

The SSE stream (`text/event-stream`, cookie or Bearer, keep-alive every 25s)
carries `document` (`{ kind, doc_id, seq }`) and `revision_pushed`
(`{ game, number }`, fired from the M2 commit path) events. Clients do not rely
on Last-Event-ID replay: on (re)connect they pull `?since_seq=<last known>` and
then stream. (`membership` events are specced but deferred in M3.)

## Test

```sh
cargo test -p server
```

Tests pass without any database running: the real-database health check and the
auth/workspace integration tests all self-skip unless `TEST_DATABASE_URL` is
set. To run them for real against the dev Postgres:

```sh
TEST_DATABASE_URL=postgres://stakedev:stakedev@localhost:5433/stakedev cargo test -p server
```
