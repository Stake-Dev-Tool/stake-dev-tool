# Contributing to Stake Dev Tool

Thanks for your interest! This project is open-source and community-driven.
Whether you want to file a bug, propose a feature, or send a patch, here's how
to get involved.

## Before you start

- Browse open [issues](https://github.com/Stake-Dev-Tool/stake-dev-tool/issues)
  and [discussions](https://github.com/Stake-Dev-Tool/stake-dev-tool/discussions)
  to see if someone is already on it.
- For non-trivial changes, open an issue first to align on the approach before
  sinking time into an implementation.

## Development setup

Prerequisites:

- **Rust** 1.90+ via `rustup`
- **Node** 20+ and **pnpm** 10+
- **Windows**: WebView2 Runtime (ships with Win 11), MSVC build tools
- **macOS**: Xcode Command Line Tools
- **Linux**: `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev
  libssl-dev libayatana-appindicator3-dev librsvg2-dev`

```bash
git clone https://github.com/Stake-Dev-Tool/stake-dev-tool.git
cd stake-dev-tool
pnpm install
pnpm tauri:dev          # hot-reloaded dev mode
```

Useful commands:

```bash
# Rust workspace check (fast, no build)
cargo check

# Run the LGS alone (no UI) — great for curl-testing the RGS endpoints
cargo run -p lgs --release

# Frontend only
pnpm --filter ui dev

# Lint / type-check the frontend
pnpm --filter ui check

# Full release bundle
pnpm tauri:build
```

## Project layout

```
crates/
├── lgs/              # Rust library + bin for the Local Game Server.
│                     # Self-contained — no desktop deps.
│                     #   • config.rs   — env-driven ServerConfig
│                     #   • routes.rs   — RGS /api/rgs/… endpoints
│                     #   • devtool.rs  — test-view tooling endpoints
│                     #   • replay.rs   — Stake Engine /bet/replay/… endpoint
│                     #   • math_engine — weighted RNG, zstd books indexing
│                     #   • session     — DashMap-backed session store
│                     #   • tls         — local CA + leaf cert (rcgen)
│                     #   • settings    — resolution presets persistence
│
└── desktop/          # Tauri app. Depends on `lgs` as a lib.
                      #   • commands.rs — Tauri IPC commands
                      #   • profiles.rs — saved game profiles
                      #   • state.rs    — running LGS handle
ui/
├── src/
│   ├── lib/          # Shared Svelte components + api client
│   │   ├── api.ts    # Tauri IPC + HTTP clients, shared types
│   │   └── Picker.svelte
│   └── routes/
│       ├── +page.svelte      # Tauri main window
│       └── test/+page.svelte # Multi-iframe test view (served by LGS)
```

## Code style

- **Rust**: `cargo fmt` + `cargo clippy --workspace --all-targets -- -D warnings`
- **TypeScript/Svelte**: Prettier defaults + `svelte-check`. No `any` unless
  justified.
- **Commits**: [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `refactor:`, `docs:`, etc.). Keep them focused and the
  subject line under 72 chars.
- Avoid adding dependencies when a standard-library solution suffices. Every
  dep is a supply-chain footprint.

## Testing

We don't have a full test suite yet. If you change the math engine or the
session store, please add a unit test next to the change.

A minimal manual smoke test:

1. `pnpm tauri:dev`
2. Drop a game folder into `./math/`
3. Install the local CA, pick the folder, click Launch
4. In the test view, run a few spins — check balance, end-round, and mode
   switching all work

## Opening a pull request

1. Fork and create a feature branch off `main`
   (`git checkout -b feat/my-change`).
2. Make your changes, commit with a Conventional Commits message.
3. Run the checks locally:
   ```bash
   cargo check --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all
   pnpm --filter ui check
   pnpm --filter ui build
   ```
4. Push and open a PR. Fill in the template — describe **what** changed and
   **why**, and list manual test steps.
5. Maintainers will review, possibly ask for changes, and merge once it looks
   good.

## Security

If you find a security issue (cert handling, RGS auth bypass, file-system
escape, etc.), **please do not open a public issue**. Email the maintainer
directly or use GitHub's private vulnerability reporting.

## License

By contributing you agree that your contributions are licensed under the
license covering the directory they touch: [MIT](LICENSE) for most of the
repository, [AGPL-3.0](crates/server/LICENSE) for `crates/server/` and
`web/`. The marketing site ([Stake-Dev-Tool/site](https://github.com/Stake-Dev-Tool/site))
is not open to external contributions.
