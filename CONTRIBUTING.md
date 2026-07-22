# Contributing to Stake Dev Tool

Thanks for your interest in making Stake Dev Tool better! Every kind of
contribution counts: bug reports, feature ideas, documentation fixes, and
code. This guide covers everything you need to go from `git clone` to a
merged pull request.

Participation in this project is covered by our
[Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to contribute

- **Report a bug** — open an issue with the
  [bug template](https://github.com/Stake-Dev-Tool/stake-dev-tool/issues/new/choose).
  Include your OS, app version, and reproduction steps; logs and screenshots
  help a lot.
- **Propose a feature** — open an issue describing the problem you're trying
  to solve (not just the solution you have in mind). Discussing before
  building saves everyone time.
- **Improve the docs** — typos, unclear steps, missing guides. Docs PRs are
  the fastest to review and merge.
- **Write code** — grab an open issue, or fix something that bit you.

**Before starting a non-trivial change, open an issue first** to align on
the approach. Small, obvious fixes (typos, broken links, clear one-line
bugs) can go straight to a PR.

## Development setup

### Prerequisites

- **Rust** 1.90+ via [`rustup`](https://rustup.rs)
- **Node.js** 20+ and **pnpm** 10+
- Per platform:
  - **Windows** — WebView2 Runtime (ships with Win 11), MSVC build tools
  - **macOS** — Xcode Command Line Tools
  - **Linux** — `libwebkit2gtk-4.1-dev build-essential curl wget file
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`

### Bootstrap

```bash
git clone https://github.com/Stake-Dev-Tool/stake-dev-tool.git
cd stake-dev-tool
pnpm install
pnpm tauri:dev          # desktop app with hot reload
```

### Working on each surface

| Surface | Command | Notes |
| --- | --- | --- |
| Desktop app | `pnpm tauri:dev` | Hot-reloaded UI + Rust rebuild on change |
| Test-view UI alone | `pnpm ui:dev` | SvelteKit dev server, no Tauri shell |
| LGS standalone | `cargo run -p lgs --release` | No UI — great for curl-testing the RGS endpoints ([env vars](docs/architecture.md#running-the-lgs-standalone)) |
| Cloud server | `cargo run -p server` | Needs Postgres + MinIO: `docker compose -f docker-compose.dev.yml up -d` — full guide in [`crates/server/README.md`](crates/server/README.md) |
| Cloud dashboard | `pnpm web:dev` | SvelteKit SPA embedded in the server |
| `sdt` CLI | `cargo build -p cli --release` | Binary lands in `target/release/sdt` |
| Desktop release build | `pnpm tauri:build` | Full bundle in `target/release/bundle/` |

### Project layout

```
crates/
├── lgs/         # The engine: RGS + devtool endpoints, math indexing, sessions, local TLS
├── desktop/     # Tauri shell: IPC commands, profiles, running-LGS state
├── server/      # Cloud server: API, auth, workspaces, revisions, share links (AGPL)
├── protocol/    # Shared request/response types, TS generated via ts-rs
└── cli/         # The sdt CLI for CI pipelines
ui/              # Workbench frontend (SvelteKit) — served by desktop or cloud
web/             # Cloud dashboard (SvelteKit SPA, embedded in the server; AGPL)
deploy/          # Production compose, Caddyfile, self-host docs
```

The full picture (diagram, HTTP API, cloud concepts) is in
[docs/architecture.md](docs/architecture.md).

## Coding standards

**Rust**

- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings`
  must pass — CI enforces both.
- Avoid adding dependencies when the standard library (or an existing dep)
  suffices. Every dependency is a supply-chain footprint.

**TypeScript / Svelte**

- Prettier defaults + `svelte-check`. No `any` unless justified.
- Types shared with Rust live in `crates/protocol` and are generated via
  `ts-rs` — never hand-write a duplicate of a protocol type.

**Comments** — explain *why*, not *what*. If a comment restates the line
below it, delete it.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(server): add per-link session caps
fix(desktop): drop redundant borrow in anyhow! format arg
docs: clarify math folder layout
refactor(lgs): extract books indexing into its own module
```

Keep commits focused and the subject line under 72 characters. The scope is
usually the crate or directory you touched (`lgs`, `desktop`, `server`,
`cli`, `ui`, `web`, `deploy`, `billing`).

## Testing

The test suite is young — that's a great contribution area in itself.

- If you change the math engine, session store, or revision logic, add a
  unit test next to the change.
- Server integration tests run against a real database and self-skip unless
  `TEST_DATABASE_URL` is set — see
  [`crates/server/README.md`](crates/server/README.md).
- Minimal manual smoke test for desktop changes:
  1. `pnpm tauri:dev`
  2. Point the app at a math folder, install the local CA, click Launch
  3. In the test view, run a few spins — check balance, end-round, and mode
     switching all work

## Submitting a pull request

1. Fork and create a feature branch off `v2`
   (`git checkout -b feat/my-change`). Active development happens on `v2`;
   `main` tracks the released V1 line.
2. Make your changes with Conventional Commits messages.
3. Run the checks locally — CI runs the same set:
   ```bash
   cargo check --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all -- --check
   pnpm --filter ui check
   pnpm --filter ui build
   ```
4. Push and open a PR. Fill in the template — describe **what** changed and
   **why**, and list manual test steps. Screenshots or a short clip for UI
   changes.
5. A maintainer reviews, possibly asks for changes, and merges once it looks
   good. Small focused PRs get reviewed much faster than big mixed ones.

## Releases (maintainers)

- **Desktop** — push a `vX.Y.Z` tag. The release workflow builds Windows,
  macOS (Apple Silicon) and Linux bundles, syncs the app version from the
  tag, signs updater artifacts with Minisign, and publishes a draft GitHub
  release. Add a [CHANGELOG.md](CHANGELOG.md) entry before tagging.
- **CLI** — push an `sdt-vX.Y.Z` tag to attach prebuilt `sdt` binaries for
  the three platforms.
- **Server** — deploys automatically from CI on push; no manual step.

## Security issues

**Please do not open a public issue for security problems** (cert handling,
RGS auth bypass, session isolation, file-system escape, billing). See
[SECURITY.md](SECURITY.md) for how to report privately.

## License of contributions

By contributing, you agree that your contributions are licensed under the
license covering the directory they touch:

- [MIT](LICENSE) for most of the repository (engine, desktop, CLI,
  protocol, UI, deploy).
- [AGPL-3.0](crates/server/LICENSE) for `crates/server/` and `web/`.

The marketing site lives in its own repo
([Stake-Dev-Tool/site](https://github.com/Stake-Dev-Tool/site)),
is all-rights-reserved, and is **not open to external contributions**.
