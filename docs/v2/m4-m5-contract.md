# M4/M5 contract — cloud LGS and share v2

Design decisions for M4 (multi-tenant LGS in the server) and M5 (share
links backed by the real RGS). Companion to [V2.md](../../V2.md) and
[recon-m3-m6.md](recon-m3-m6.md) §B. The lgs-side plumbing (TenantId,
TenantRegistry, shared BooksCache, MathSource) already landed on `v2`.

## M4 — multi-tenant LGS in `crates/server`

### Materialization (object store → local disk)

The LGS reads math from disk (mmap), so a revision must be materialized
before play:

- Cache dir: `<STORAGE_FS_ROOT>/../cache/rev/<workspace_id>/<game_id>/<number>/<game_slug>/…`
  (the trailing `<game_slug>/` level exists because the LGS resolves
  `math_root/<game>/file`).
- Files stream from the object store by manifest, sha256-verified on write;
  a `.complete` marker makes materialization idempotent and crash-safe.
- Eviction: LRU by directory atime with a byte budget
  (`SERVER_MATH_CACHE_BYTES`, default 20 GiB). Materialization is
  on-demand at first session, with a per-revision async lock so concurrent
  requests wait on one download.

### Tenancy & sessions

- `TenantId` string: `ws:<workspace_id>:game:<game_id>:rev:<number>` —
  opaque to lgs, parseable by the server.
- `TenantRegistry.get_or_create_disk(tenant, dir)` with in-memory sessions
  (already the registry default). Per-plan books cap via
  `set_tenant_cap` keyed on the workspace (config now, billing later).
- Workbench sessions are authenticated: the server namespaces LGS session
  ids as `<user_id>:<client-provided id>` so users never collide; the
  devtool "reset sessions" surface therefore only ever touches one
  tenant's store (the recon's global-reset hazard is structurally gone).

### Routing

Mount under the authed API, path-scoped to the revision:

```
/api/ws/:slug/g/:game/r/:number/{rgs,devtool,bet}/*rest
```

Handler: resolve membership (404 for non-members), resolve the revision,
ensure materialized, `registry.router_for(tenant)` (cached
`Router → Service` per tenant), strip the prefix, forward. The inner LGS
surface stays byte-identical to standalone (`/api/rgs/<game>/wallet/…`,
`/api/devtool/…`), which is what makes M6 a thin re-base (recon B.4).

SSE note: the devtool stream authenticates via the session cookie
(same-origin), like every other workbench call.

**Done when** (V2.md): an authenticated browser session plays a workspace
game on the cloud LGS at a pinned revision — proven by an integration test
that prepares a session and runs `authenticate/balance/play/end-round`
through the mounted tenant router.

## M5 — Share v2

### Model

```sql
CREATE TABLE front_bundles (         -- game front build, pushed like math
    id UUID PK, game_id UUID FK CASCADE,
    manifest JSONB NOT NULL,         -- path → {hash, size}; index.html required
    created_by UUID, created_at TIMESTAMPTZ
);
CREATE TABLE share_links (
    id UUID PK, workspace_id FK, game_id FK CASCADE,
    slug TEXT NOT NULL UNIQUE,       -- subdomain: <slug>.play.<domain>
    revision_number INTEGER,         -- NULL = track latest
    front_bundle_id UUID FK,         -- NULL = latest bundle
    password_hash TEXT,              -- argon2, NULL = public
    expires_at TIMESTAMPTZ,          -- NULL = never
    max_concurrent_sessions INTEGER NOT NULL DEFAULT 25,
    revoked_at TIMESTAMPTZ, created_by UUID, created_at TIMESTAMPTZ,
    sessions_count BIGINT NOT NULL DEFAULT 0,   -- lifetime counters
    spins_count BIGINT NOT NULL DEFAULT 0,
    total_bet NUMERIC NOT NULL DEFAULT 0, total_win NUMERIC NOT NULL DEFAULT 0
);
```

- Front bundles reuse the M2 blob machinery verbatim (same content-addressed
  store, same check/upload flow) — `sdt push-front <dist-dir> --game …`.
- Share slugs: generated `word-word-nnn` by default, custom on paid plans.

### Host routing (the wildcard)

Caddy terminates TLS for `*.play.<domain>` (already staged in
deploy/Caddyfile) and proxies with the Host header intact. In axum, a
Host-based layer runs BEFORE the app router: requests whose host matches
`*.play.<PLAY_DOMAIN>` are dispatched to the share router, everything else
falls through to the app (app cookies can never be read there — different
registrable domain configured via `SERVER_PLAY_DOMAIN`).

Share router, per resolved link (404 page when unknown/revoked/expired):
- `/` + static paths → bundle files from the object store (same streaming
  as M2 file downloads, `index.html` fallback), long immutable cache
  headers keyed by bundle id.
- `/api/rgs/<game_slug>/*` → the LGS tenant for (game, pinned-or-latest
  revision) — the exact same tenant machinery as M4.
- Password-protected links: an interstitial page POSTs the password; on
  success a short-lived `sdt_share` cookie (scoped to that exact
  subdomain) unlocks assets + RGS.
- Visitor sessions: anonymous LGS sessions namespaced `visitor:<random>`;
  per-link concurrent cap (429 + friendly page beyond), per-IP rate
  limits on session creation and spins.

### Analytics & dashboard API

Counters update in the wallet path (cheap atomic SQL increments, observed
RTP = total_win / total_bet). Dashboard CRUD:

```
POST   /api/workspaces/:slug/games/:game/shares          → link (+ full URL)
GET    /api/workspaces/:slug/games/:game/shares          → list + counters
PATCH  /api/workspaces/:slug/games/:game/shares/:id      → pin/expiry/password/revoke
DELETE /api/workspaces/:slug/games/:game/shares/:id
POST   /api/workspaces/:slug/games/:game/front-bundles/… → check/upload/commit (M2 flow)
```

**Done when** (V2.md): a tester with just the URL plays against the real
RGS and the owner sees play counts. The WASM share path retires: the
desktop "share" flow creates cloud links instead of GitHub Pages deploys
(the old code is deleted in the desktop cutover, and `lgs-wasm` leaves the
critical path).

## Sequencing note

M4 lands first (it unblocks both M5's RGS and M6's workbench). M5's
front-bundle push + host routing can start the moment M2's blob flow is
merged, in parallel with M4's tenant router if file ownership is split:
M4 = `crates/server/src/lgs_host/` (materialize + registry + mount), M5 =
`crates/server/src/share/` (+ migration + Caddy uncomment), meeting only
at the router registration lines.
