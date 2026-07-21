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
| `RUST_LOG` | `info` | `tracing` env-filter directive. |
| `TEST_DATABASE_URL` | _(unset)_ | Enables the real-database integration test when set. |

## Test

```sh
cargo test -p server
```

Tests pass without any database running: the real-database health check
self-skips unless `TEST_DATABASE_URL` is set.
