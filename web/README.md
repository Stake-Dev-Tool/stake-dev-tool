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
- **Routes:** `/login`, `/` (workspaces), `/w/[slug]` (members + invites),
  `/invite/[token]` (public accept), `/device` (device-code approval),
  `/account` (API tokens + logout).
