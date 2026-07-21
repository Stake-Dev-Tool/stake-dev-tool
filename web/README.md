# web — Stake Cloud dashboard

The cloud platform's dashboard (front half of **M1: Identity & workspaces**). A
SvelteKit 2 + Svelte 5 SPA, built with `@sveltejs/adapter-static` and served by
the axum `server` binary (same pattern the desktop app uses for the test view).

Dark-by-default, single emerald accent, no component library — a small set of
shared primitives lives in `src/lib/components/`.

## Dev workflow

The dashboard needs the API. Run the server and the dev UI side by side:

```sh
# terminal 1 — the API (from repo root)
cargo run -p server          # listens on http://127.0.0.1:8080

# terminal 2 — the dashboard (from repo root)
pnpm --filter web dev        # http://localhost:5190
```

Vite proxies `/api/*` → `http://127.0.0.1:8080`, so the browser only ever sees
one origin (`localhost:5190`) and the session cookie stays same-origin in dev,
exactly as in production. The UI degrades gracefully when the server is down
(network errors surface as inline messages), so you can work on it without the
backend running.

## Build

```sh
pnpm --filter web build      # → web/build/
pnpm --filter web check      # svelte-check, 0 errors
```

Output is a static SPA in **`web/build/`** with an `index.html` fallback.
`ssr = false` and nothing is prerendered, so every route — including deep links
like `/w/:slug` and `/invite/:token` — resolves to the fallback and hydrates
client-side.

**Integration note (axum):** mount `web/build/` as the static root for
`app.<domain>` and serve `index.html` as the fallback for any path that isn't a
static asset or `/api/*`. That fallback is what makes deep links work.

## Architecture notes

- **`src/lib/api.ts`** — the single source of all fetch logic and request/
  response TypeScript types, hand-written against the M1 contract. Responses are
  parsed defensively (`normalize*` helpers) so nested-vs-flat shape differences
  are absorbed in one place. **To be reconciled at integration** with the
  generated bindings in `ui/src/lib/protocol` (ts-rs output from
  `crates/protocol`) — when field names shift, only this file changes.
- **`src/lib/session.svelte.ts`** — `$state` auth store; `refreshSession()`
  calls `/api/auth/me` once. The root `+layout.svelte` runs the client-side
  auth guard and redirects unauthenticated users to `/login?next=…`. `/login`
  and `/invite/:token` are the only public routes.
- **Routes:** `/login`, `/` (workspaces), `/w/[slug]` (games + members +
  invites), `/w/[slug]/g/[game]` (revision list), `/w/[slug]/g/[game]/r/[number]`
  (revision detail), `/w/[slug]/g/[game]/diff/[a]/[b]` (revision diff),
  `/invite/[token]` (public accept), `/device` (device-code approval),
  `/account` (API tokens + logout).

## Games & revisions

The `/w/[slug]/g/*` routes surface **M2 math revisions**. Reads are unchanged;
revisions can now also be **pushed straight from the browser** (see below) as
well as from CI via `sdt push`. The workspace page lists games (name, slug,
head-revision badge); a game page shows its revisions newest-first (message,
author, age, file count, size, and a bet-stats badge) with a two-select
**Compare** picker into the diff view. A revision page shows its file manifest
(path, human size, copyable short hash) and the server-computed bet-stats table
(mode, cost, RTP as a percentage, max win as a ×multiplier, entries); while
stats are `pending` it **polls the detail endpoint every 3s** (an `$effect`
teardown stops the poll on `ok`/`error` and on unmount). The diff view
(`/diff/[a]/[b]`, `a` = after, `b` = before) renders file add/remove/change
chips and per-mode before→after stats with a signed RTP delta in percentage
points. All wire shapes live behind `normalize*` helpers in `api.ts`
(`api.games.*`) exactly like the M1 surface, ready to reconcile against the
generated `crates/protocol` bindings at integration.

## Browser push

A revision can be pushed from the dashboard without leaving the browser — the
same content-addressed flow `sdt push` uses. Session-cookie auth already carries
the implicit `full` scope (which satisfies `push:math`), so no token is needed.

- **Where:** a **Push a revision** button on the game page (`/w/[slug]/g/[game]`)
  pushes into that game; a **New game** button on the workspace Games card runs
  the same flow plus a live-derived, validated game-slug input
  (`^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$`). CI (`sdt push`) remains available and is
  offered alongside in every empty state.
- **Intake** (`src/lib/components/MathFolderPicker.svelte`): drag a folder
  (walked via `DataTransferItem.webkitGetAsEntry`) or click to browse
  (`<input webkitdirectory>`). Paths are made relative POSIX with the top folder
  stripped; dotfiles are skipped. Rejects a folder with no root `index.json`,
  more than 1000 files, or nothing usable, and shows a summary (file count, total
  size, largest file).
- **Pipeline** (`src/lib/push.ts`, UI-agnostic + unit-testable): sha256 each file
  with `hash-wasm`, streamed from `file.stream()` in chunks so a multi-GB book is
  never buffered whole → `POST …/revisions/check` for the missing hashes → `PUT
  …/blobs/:hash` for each missing blob (unique by hash; the `File` is the fetch
  body, so uploads stream too; **3 concurrent**) → `POST …/revisions`. A 409
  `missing_blobs` at commit re-uploads exactly the named hashes and retries the
  commit **once**. `parent_number` is the game's head (null for a new game).
- **Progress:** per-file states (queued → hashing % → uploaded / deduplicated)
  and a global recap (**x / y files, z sent, w deduplicated**). Upload progress is
  per-file (start/done) — `fetch` does not expose sub-file upload progress.
- **Errors** map to precise copy: 413 → "larger than the server allows",
  `hash_mismatch`, `stale_parent` → "someone pushed meanwhile — reload",
  `invalid_manifest` → the server's message.
- On success the flow navigates to the new revision's page, where stats poll.

`ApiError` now carries a `details` field (the parsed error body) so the commit
handler can read the top-level `missing` array a 409 returns. Adds one runtime
dependency, **`hash-wasm`**.
