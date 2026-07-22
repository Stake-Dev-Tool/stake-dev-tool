<h1 align="center">Stake Dev Tool</h1>

<p align="center">
  The open-source workbench and cloud platform for slot games on the
  <a href="https://stake-engine.com/">Stake Engine</a> RGS contract.<br />
  Run, debug and QA your slot locally, sync math and rounds with your team,
  and share real playable links backed by a server-side RGS.
</p>

<p align="center">
  <a href="https://github.com/simnJS/stake-dev-tool/releases/latest">
    <img alt="Latest release" src="https://img.shields.io/github/v/release/simnJS/stake-dev-tool?style=flat-square&color=emerald" />
  </a>
  <a href="https://github.com/simnJS/stake-dev-tool/releases">
    <img alt="Total downloads" src="https://img.shields.io/github/downloads/simnJS/stake-dev-tool/total?style=flat-square&color=blue" />
  </a>
  <a href="https://github.com/simnJS/stake-dev-tool/actions/workflows/ci.yml">
    <img alt="CI" src="https://img.shields.io/github/actions/workflow/status/simnJS/stake-dev-tool/ci.yml?branch=main&label=CI&style=flat-square" />
  </a>
  <a href="LICENSE">
    <img alt="License" src="https://img.shields.io/github/license/simnJS/stake-dev-tool?style=flat-square" />
  </a>
</p>

<p align="center">
  <a href="https://stakedevtool.com"><b>Website</b></a> ·
  <a href="https://app.stakedevtool.com"><b>Cloud dashboard</b></a> ·
  <a href="https://github.com/simnJS/stake-dev-tool/releases/latest"><b>Download</b></a> ·
  <a href="deploy/README.md"><b>Self-host</b></a> ·
  <a href="V2.md"><b>V2 plan</b></a>
</p>

---

## One engine, three surfaces

The same Rust engine (`crates/lgs`) powers everything:

1. **Desktop app** — the local dev loop: front hot-reload, local math,
   instant restarts. Free and MIT forever.
2. **Web workbench** — the workbench served from the cloud. Math devs, QA
   and PMs get it in a browser with zero install.
3. **Share links** — every link is a real hosted game instance on its own
   `<slug>.play.` subdomain, playing against a real server-side RGS. Your
   math files never leave the server.

The entire platform is open source and fully self-hostable with every
feature included. The optional subscription at
[stakedevtool.com](https://stakedevtool.com/pricing) sells hosting on our
infrastructure, nothing else. See [V2.md](V2.md) for the architecture and
roadmap.

## Get started

**Use the hosted cloud (zero install).** Create an account at
[app.stakedevtool.com](https://app.stakedevtool.com) and pick a plan
([pricing](https://stakedevtool.com/pricing)) — or self-host for free.

**Install the desktop app.** Grab the latest build from the
[Releases page](https://github.com/simnJS/stake-dev-tool/releases/latest):

| Platform                   | File                                            | Notes                                       |
| -------------------------- | ----------------------------------------------- | ------------------------------------------- |
| Windows 10/11 (x64)        | `Stake-Dev-Tool-vX.Y.Z-windows-x64.exe`         | NSIS installer                              |
| macOS Apple Silicon        | `Stake-Dev-Tool-vX.Y.Z-macos-arm64.app.tar.gz`  | Extract, then see [macOS first launch](#macos-first-launch) |
| Debian / Ubuntu (x64)      | `Stake-Dev-Tool-vX.Y.Z-linux-x64.deb`           | `sudo apt install ./<file>.deb`             |
| Other Linux (x64)          | `Stake-Dev-Tool-vX.Y.Z-linux-x64.AppImage`      | `chmod +x` then run                         |

> Intel Macs aren't supported — open an issue if that's a blocker.

**Self-host the cloud platform.** One Linux box, Docker, Postgres and
Caddy — the same stack we run in production:

```bash
git clone https://github.com/simnJS/stake-dev-tool.git && cd stake-dev-tool/deploy
cp .env.prod.example .env.prod && $EDITOR .env.prod
docker compose -f docker-compose.prod.yml --env-file .env.prod up -d --build
```

Full walkthrough (DNS, TLS, backups, updates) in
[`deploy/README.md`](deploy/README.md).

### macOS first launch

The macOS build is not yet signed with an Apple Developer ID, so on first
launch Gatekeeper shows:

> "Stake Dev Tool.app" is damaged and can't be opened. You should move it
> to the Bin.

The app **isn't** damaged — macOS just blocks unsigned downloads. To unblock
it, run this once in Terminal after moving the app to `/Applications`:

```bash
xattr -dr com.apple.quarantine "/Applications/Stake Dev Tool.app"
```

Alternative one-time bypass: right-click the app → **Open** → confirm
**Open** in the dialog.

## Features

### The local loop (desktop)

- **Fast Rust LGS** — drop-in `/api/rgs/<game>/wallet/…` server. Reads
  `index.json` + `lookuptable_*.csv` + `books_*.jsonl.zst` from disk, indexes
  books once per mode, weighted RNG via binary search.
- **Multi-resolution test view** — run your game side-by-side at 7 built-in
  resolutions plus any custom sizes. Each iframe is its own session.
- **Live event stream** — SSE pushes every spin to the test view, with bet
  history + last-event strip per frame.
- **Force / replay / bookmark** — pin any `(mode, eventId)`, replay a saved
  outcome, bookmark notable rounds (auto-picked min / avg / max per mode).
- **Local HTTPS** — bundled CA installs into your user trust store. Zero browser
  warnings, no game-code hacks.
- **Profiles** — math folder + front URL + resolution snapshot saved per game,
  one-click reload.
- **Auto-updater** — Minisign-signed releases, silent install on Windows,
  replace-in-place on macOS/Linux.

### The cloud platform (new in V2)

- **Workspaces** — accounts, roles (owner / admin / member) and email
  invites. Games, quotas and sync all hang off the workspace.
- **Math revisions** — immutable snapshots with content-addressed dedup: a
  books file unchanged between rev 41 and 42 is never re-uploaded. Every
  push gets an automatic changelog against the previous revision (RTP per
  mode, max win, modes added/removed).
- **`sdt` CLI for CI** — math is generated by simulations, so the real
  workflow is CI pushing a revision: `sdt push ./math/my-game`. Device-flow
  login, scoped API tokens, only changed blobs upload. See
  [`crates/cli`](crates/cli/).
- **Team sync v2** — profiles, saved rounds and bookmarks sync through the
  workspace, live over SSE. Replays reference `(revision, mode, eventId)`,
  so a math push never breaks a bookmark.
- **Share links v2** — real hosted game instances on `<slug>.play.`
  subdomains: same-origin front + RGS, pinned or tracking revisions, expiry
  and password options, per-link analytics (sessions, spins, observed RTP).
- **Web workbench** — the multi-resolution test view served from the cloud,
  no install.

> Upgrading from V1: the GitHub-repo Teams and the WASM/GitHub Pages share
> are replaced by cloud workspaces and share links v2, with a migration
> path for existing teams. V1 keeps working in the meantime.

## Quick start (desktop)

1. **Launch the app** and click **Install Local CA** in the amber banner. One
   prompt on macOS, silent on Windows; on Linux the `.deb` pulls
   `libnss3-tools` automatically (AppImage users: `sudo apt install
   libnss3-tools`). Firefox uses its own store — trust manually if needed.
2. **Browse…** to your game's math folder.
3. Enter the **Front URL** of your game's frontend (e.g. `http://localhost:5174`).
4. **Launch test view** — a Chromium window opens with your game at every
   enabled resolution.
5. **Save** the profile to reload it in one click next time.

The test view sidebar covers balance, currency, language, device, social mode,
custom resolutions, force / bookmark / replay, and per-frame mute.

## Math folder layout

```
<math_root>/
└── <game-slug>/
    ├── index.json            # { "modes": [{ "name", "cost", "events", "weights" }, …] }
    ├── lookuptable_<mode>.csv     # eventId,weight,payoutMultiplier
    └── books_<mode>.jsonl.zst     # one event per line, zstd-compressed
```

Modes are auto-detected from `index.json`.

## Architecture

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

One `pnpm` workspace, one Cargo workspace:

- [`crates/lgs`](crates/lgs/) — the engine: LGS library + standalone binary,
  embeddable and multi-tenant.
- [`crates/desktop`](crates/desktop/) — Tauri shell + commands.
- [`crates/server`](crates/server/) — the cloud server: API, auth,
  workspaces, revisions, embedded dashboard.
- [`crates/protocol`](crates/protocol/) — shared request/response types,
  TypeScript generated via `ts-rs`.
- [`crates/cli`](crates/cli/) — the `sdt` CLI (`login`, `push`, `revisions`).
- [`ui/`](ui/) — the workbench frontend (SvelteKit), served by desktop or cloud.
- [`web/`](web/) — the cloud dashboard (SvelteKit SPA), embedded in the server.
- [`site/`](site/) — the marketing site at
  [stakedevtool.com](https://stakedevtool.com) (TanStack Start).
- [`deploy/`](deploy/) — production compose file, Caddyfile, self-host docs.

## HTTP endpoints

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

## Run the LGS standalone

```bash
LGS_BIND_ADDR=127.0.0.1:3001 \
LGS_MATH_DIR=./math \
cargo run -p lgs --release
```

| Variable        | Default        | Purpose                                |
| --------------- | -------------- | -------------------------------------- |
| `LGS_BIND_ADDR` | `0.0.0.0:3001` | Where the LGS binds                    |
| `LGS_MATH_DIR`  | `./math`       | Root folder of game subfolders         |
| `LGS_UI_DIR`    | auto-detected  | Override path to `ui/build/`           |
| `RUST_LOG`      | `info`         | `tracing-subscriber` filter            |

## Build from source

**Prerequisites**

- Rust 1.90+ (rustup)
- Node.js 20+ and pnpm 10+
- Windows: [WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (pre-installed on Win 11)
- macOS: Xcode Command Line Tools
- Linux: `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`

```bash
git clone https://github.com/simnJS/stake-dev-tool.git
cd stake-dev-tool
pnpm install

pnpm tauri:dev      # desktop app, hot reload
pnpm tauri:build    # desktop release build → target/release/bundle/

cargo run -p server            # cloud server (needs Postgres, see deploy/)
pnpm web:dev                   # cloud dashboard
pnpm site:dev                  # marketing site
cargo build -p cli --release   # sdt CLI → target/release/sdt
```

## Auto-updater

The desktop app checks GitHub Releases on startup and shows a banner when a
newer version is published. Updates are Minisign-verified and installed
silently (passive NSIS on Windows, replace-in-place elsewhere).

Releases are signed via the GitHub Actions workflow on every `v*` tag — see
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the maintainer setup.

## Contributing

Issues, PRs, and discussions are welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

This is one repository with three licensing zones:

| Path | License |
|---|---|
| Everything not listed below — desktop app, `lgs` engine, CLI, protocol, test-view UI, deploy configs | **MIT** — [LICENSE](LICENSE) |
| `crates/server/`, `web/` (the cloud server + its dashboard) | **AGPL-3.0** — [crates/server/LICENSE](crates/server/LICENSE) |
| `site/` (the stakedevtool.com marketing site) | Source-visible, **all rights reserved** — [site/LICENSE](site/LICENSE) |

Self-hosting the server is free forever and untouched by the AGPL — the
licence only requires anyone offering a modified server as a hosted
service to publish their changes.

---

<p align="center">
  <sub>
    Built by <a href="https://github.com/simnJS">@simnJS</a> ·
    <a href="https://stakedevtool.com">stakedevtool.com</a> ·
    <a href="CHANGELOG.md">Changelog</a> ·
    <a href="https://github.com/simnJS/stake-dev-tool/issues/new">Report a bug</a>
  </sub>
</p>
