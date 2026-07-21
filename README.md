<h1 align="center">Stake Dev Tool</h1>

<p align="center">
  Desktop workbench for slot games on the
  <a href="https://stake-engine.com/">Stake Engine</a> RGS contract.<br />
  Run, debug, and visually QA your slot locally — multi-resolution test view,
  fast Rust LGS, team sync, one-click shareable preview links.
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
  <img alt="Rust" src="https://img.shields.io/badge/Rust-1.90%2B-CE412B?style=flat-square&logo=rust&logoColor=white" />
  <img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8DB?style=flat-square&logo=tauri&logoColor=white" />
  <img alt="SvelteKit" src="https://img.shields.io/badge/SvelteKit-5-FF3E00?style=flat-square&logo=svelte&logoColor=white" />
</p>

---

## Download

Grab the latest build from the [Releases page](https://github.com/simnJS/stake-dev-tool/releases/latest):

| Platform                   | File                                            | Notes                                       |
| -------------------------- | ----------------------------------------------- | ------------------------------------------- |
| Windows 10/11 (x64)        | `Stake-Dev-Tool-vX.Y.Z-windows-x64.exe`         | NSIS installer                              |
| macOS Apple Silicon        | `Stake-Dev-Tool-vX.Y.Z-macos-arm64.app.tar.gz`  | Extract, then see [macOS first launch](#macos-first-launch) |
| Debian / Ubuntu (x64)      | `Stake-Dev-Tool-vX.Y.Z-linux-x64.deb`           | `sudo apt install ./<file>.deb`             |
| Other Linux (x64)          | `Stake-Dev-Tool-vX.Y.Z-linux-x64.AppImage`      | `chmod +x` then run                         |

> Intel Macs aren't supported — open an issue if that's a blocker.

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

Then double-click the app as usual. If your copy is named differently
(e.g. `Stake Dev Tool 2.app`), use that exact name in the path. Auto-updates
applied later don't re-trigger the warning.

Alternative one-time bypass: right-click the app → **Open** → confirm
**Open** in the dialog. This only works on the very first launch;
afterwards macOS remembers your decision.

## Features

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
- **Teams** *(deprecated)* — collaborative workspaces backed by a private
  GitHub repo. Sync profiles + saved rounds + math files (chunked Release
  assets). Replaced by cloud workspaces in V2 (see [V2.md](V2.md)); still
  works, with a migration path planned.
- **Share preview link** *(deprecated)* — one click ships your game as a
  static page on GitHub Pages. Math + game run in the browser via a WASM RGS —
  no server, anyone with the URL can play. Three privacy modes
  (Sampled / Partial / Full). Replaced in V2 by hosted share links backed by a
  real server-side RGS (see [V2.md](V2.md)); existing links keep working.
- **Profiles** — math folder + front URL + resolution snapshot saved per game,
  one-click reload.
- **Auto-updater** — Minisign-signed releases, silent install on Windows,
  replace-in-place on macOS/Linux.

## Quick start

1. **Launch the app** and click **Install Local CA** in the amber banner. One
   prompt on macOS, silent on Windows; on Linux the `.deb` pulls
   `libnss3-tools` automatically (AppImage users: `sudo apt install
   libnss3-tools`). Firefox uses its own store — trust manually if needed.
2. **Browse…** to your game's math folder.
3. Enter the **Front URL** of your game's frontend (e.g. `http://localhost:5174`).
4. **Launch test view** — a Chromium window opens on
   `https://localhost:<port>/test/…` (default port 3001, configurable from the
   topbar Settings) with your game at every enabled resolution.
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
┌─ Tauri desktop app ───────────────────────────────────┐
│  WebView2 UI (SvelteKit)  ←IPC→  Rust commands        │
│                                    │                   │
│                          Embedded LGS (axum + rustls)  │
└──────────────────────────────────┬────────────────────┘
                                   │ HTTPS (local CA)
                                   ▼
                       External Chromium (--max-active-webgl-contexts=64)
                       → iframe × N (one per resolution)
```

The test view is **served by the LGS** (not Tauri's custom protocol) so iframes
load from a real browser context with full WebGL support.

### Crates

- [`crates/lgs`](crates/lgs/) — LGS library + standalone binary. Run without
  the desktop via `cargo run -p lgs --release`.
- [`crates/desktop`](crates/desktop/) — Tauri shell + commands.
- [`crates/lgs-wasm`](crates/lgs-wasm/) — math engine compiled to wasm32 for
  browser-hosted preview links (deprecated along with the share path).
- [`ui/`](ui/) — SvelteKit frontend, builds to static HTML/JS.

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

pnpm tauri:dev      # hot-reload dev build
pnpm tauri:build    # release build → target/release/bundle/
```

## Auto-updater

The app checks GitHub Releases on startup and shows a banner when a newer
version is published. Updates are Minisign-verified and installed silently
(passive NSIS on Windows, replace-in-place elsewhere).

Releases are signed via the GitHub Actions workflow on every `v*` tag — see
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the maintainer setup.

## Contributing

Issues, PRs, and discussions are welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).

---

<p align="center">
  <sub>
    Built by <a href="https://github.com/simnJS">@simnJS</a> ·
    <a href="CHANGELOG.md">Changelog</a> ·
    <a href="https://github.com/simnJS/stake-dev-tool/issues/new">Report a bug</a>
  </sub>
</p>
