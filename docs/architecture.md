# Architecture

One Rust engine (`crates/lgs`) powers three surfaces: the desktop app, the
web workbench, and hosted share links. This page is the map; the full V2
design rationale lives in [V2.md](../V2.md).

```
Local                                     Cloud (hosted or self-hosted)
┌─ Tauri desktop app ─────────────┐       ┌─ Caddy (TLS, *.play wildcard) ──┐
│  SvelteKit UI ←IPC→ Rust        │       │  crates/server (axum)           │
│  Embedded LGS (axum + rustls)   │       │  API + auth · dashboard         │
└───────────────┬─────────────────┘       │  multi-tenant LGS · share router│
                │ HTTPS (local CA)        └───┬──────────┬─────────┬────────┘
                ▼                             │          │         │
     External Chromium                   Postgres   blob cache  object store
     → iframe × N (one per resolution)   (sqlx)     (mmap LRU)  (fs / S3-R2)
```

## Repository layout

One `pnpm` workspace, one Cargo workspace:

| Path | What it is |
| --- | --- |
| [`crates/lgs`](../crates/lgs/) | The engine: LGS library + standalone binary, embeddable and multi-tenant. |
| [`crates/desktop`](../crates/desktop/) | Tauri shell + IPC commands. |
| [`crates/server`](../crates/server/) | The cloud server: API, auth, workspaces, revisions, embedded dashboard. |
| [`crates/protocol`](../crates/protocol/) | Shared request/response types, TypeScript generated via `ts-rs`. |
| [`crates/cli`](../crates/cli/) | The `sdt` CLI (`login`, `push`, `revisions`) for CI pipelines. |
| [`ui/`](../ui/) | The workbench frontend (SvelteKit), served by desktop or cloud. |
| [`web/`](../web/) | The cloud dashboard (SvelteKit SPA), embedded in the server. |
| [`site/`](../site/) | The marketing site at [stakedevtool.com](https://stakedevtool.com) (TanStack Start). |
| [`deploy/`](../deploy/) | Production compose file, Caddyfile, self-host docs. |

## The engine

`crates/lgs` is a drop-in `/api/rgs/<game>/wallet/…` server implementing the
Stake Engine RGS contract. It reads `index.json` + `lookuptable_*.csv` +
`books_*.jsonl.zst` from disk, indexes books once per mode (mmap with LRU
eviction), and draws outcomes with weighted RNG via binary search. The same
crate is embedded by the desktop app (single tenant) and the cloud server
(multi-tenant, sessions pinned to a math revision).

## HTTP API

**RGS contract** (Stake Engine compatible)

```
POST /api/rgs/<game>/wallet/{authenticate,balance,play,end-round}
POST /api/rgs/<game>/bet/event
GET  /bet/replay/<game>/<version>/<mode>/<event>
```

**Devtool** (test view + desktop, no auth)

```
GET    /api/devtool/status
POST   /api/devtool/sessions/prepare
GET    /api/devtool/sessions/<sid>/{last-event,events,stream}     ← SSE
GET    /api/devtool/games/<game>/modes
GET    /api/devtool/bet-stats/<game>
GET    /api/devtool/saved-rounds                                  (POST + PATCH/DELETE :id)
GET    /api/devtool/settings                                      (POST toggle, custom + DELETE :id)
GET    /api/devtool/force-event                                   (POST + DELETE)
```

## Running the LGS standalone

Useful for curl-testing the RGS endpoints or serving math without the app:

```bash
LGS_BIND_ADDR=127.0.0.1:3001 \
LGS_MATH_DIR=./math \
cargo run -p lgs --release
```

| Variable        | Default        | Purpose                        |
| --------------- | -------------- | ------------------------------ |
| `LGS_BIND_ADDR` | `0.0.0.0:3001` | Where the LGS binds            |
| `LGS_MATH_DIR`  | `./math`       | Root folder of game subfolders |
| `LGS_UI_DIR`    | auto-detected  | Override path to `ui/build/`   |
| `RUST_LOG`      | `info`         | `tracing-subscriber` filter    |

## The cloud platform

The server (`crates/server`) adds on top of the engine:

- **Workspaces** — users, roles (owner / admin / member), email invites.
  Games, quotas and sync all hang off the workspace.
- **Math revisions** — immutable snapshots with content-addressed blob
  dedup: pushes upload only changed files, and every revision gets an
  automatic changelog against its parent (RTP per mode, max win, modes
  added/removed).
- **Document sync** — profiles, saved rounds and bookmarks sync through the
  workspace, live over SSE. Replays reference `(revision, mode, eventId)`,
  so a math push never breaks a bookmark.
- **Share links** — real hosted game instances on `<slug>.play.` subdomains:
  same-origin front + RGS, pinned or tracking revisions, expiry and password
  options, per-link analytics. Math files never leave the server.

Deep dives: [`crates/server/README.md`](../crates/server/README.md) for
configuration and development, [V2.md](../V2.md) for the design decisions,
[`deploy/README.md`](../deploy/README.md) for running it in production.
