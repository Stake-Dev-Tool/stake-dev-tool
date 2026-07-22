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
  invites), `/w/[slug]/billing` (plan + usage + upgrade),
  `/w/[slug]/g/[game]` (revision list), `/w/[slug]/g/[game]/r/[number]`
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

## Test view (M6)

The multi-resolution **test view** — the same page the desktop app embeds — runs
against the cloud LGS straight from the dashboard. An **Open test view** button on
the game page (targets the head revision) and on a revision detail page (that
revision) opens a small **FrontUrlDialog** that collects the game-front URL
(remembered per game in `localStorage`, warns when an `http://` front would be
blocked as mixed content on the https dashboard, and hints that hosted front
bundles arrive with share links in M5). It then opens, in a new tab:

```
/api/ws/<slug>/g/<game>/r/<number>/test/?gameSlug=<game>&gameUrl=<front>
```

That path is served same-origin by the M4 tenant router (the LGS embeds the built
test view), so the session cookie authorizes it. The test view detects the tenant
prefix off its own `location.pathname` and re-bases every devtool/RGS/replay call
under it — see `ui/src/lib/testview/context.ts` (`resolveContext`); no desktop
regression (the prefix-less desktop path is byte-identical). Front bundles are
brought by the tester for now; M5 will host them same-origin.

## Browser push

A revision can be pushed from the dashboard without leaving the browser — the
same content-addressed flow `sdt push` uses. Session-cookie auth already carries
the implicit `full` scope (which satisfies `push:math`), so no token is needed.

- **Where:** a single **Push** button on the game page (`/w/[slug]/g/[game]`)
  opens the unified **`PushPanel`**, which auto-detects the kind from the dropped
  folder's root — `index.json` ⇒ a math **revision** (message required; commits a
  revision, then navigates to it), `index.html` ⇒ a **front bundle** (no message;
  success shows a toast, no navigation), **both** present ⇒ a small radio to
  choose, **neither** ⇒ a clear error naming both. A **New game** button on the
  workspace Games card still runs the math-only flow (`MathPushPanel`) plus a
  live-derived, validated game-slug input (`^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$`).
  CI (`sdt push` / `sdt push-front`) remains available and is offered alongside in
  every empty state.
- **Intake** (`src/lib/components/MathFolderPicker.svelte`): drag a folder
  (walked via `DataTransferItem.webkitGetAsEntry`) or click to browse
  (`<input webkitdirectory>`). Paths are made relative POSIX with the top folder
  stripped; dotfiles are skipped, and it shows a summary (file count, total size,
  largest file). The required root file is a prop — `index.json` (math, default),
  `index.html` (front), or the sentinel **`"detect"`** used by `PushPanel`, which
  accepts either root and reports which kinds fit via a third `onpicked` argument
  (`{ math, front }`). Per-kind caps apply after detection (math 1000, front
  2000); a folder that has both roots but overflows one cap stays valid for the
  other kind rather than being rejected outright.
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
- On success a math push navigates to the new revision's page, where stats poll;
  a front push stays put and shows a toast (shares and the test view pick up the
  latest bundle automatically).

`ApiError` now carries a `details` field (the parsed error body) so the commit
handler can read the top-level `missing` array a 409 returns. Adds one runtime
dependency, **`hash-wasm`**.

## Billing & plans (M7)

Stripe-backed subscriptions. On a billing-enabled instance a workspace is
read-only (the **Free** state) until it subscribes; self-hosting stays fully
unlimited. The paywall moments are exactly two: the no-active-plan banner and
quota-hit errors. The whole surface degrades to **nothing** on a self-hosted
instance (`GET /billing` → `enabled: false`, every limit unlimited).

- **Client** (`src/lib/api.ts`, `api.billing.*`): `status(slug)` (member-visible,
  always reachable) and `checkout(slug, plan, interval)` (owner-only; returns the
  hosted Stripe Checkout URL to navigate to). Wire shapes go through `normalize*` helpers
  like the rest of the surface — reconciled against the generated
  `crates/protocol` bindings (`BillingStatusResponse`, `BillingUsage`,
  `BillingLimits`, `CheckoutRequest/Response`, `PlanId`, `BillingInterval`).
  `bigint` usage counts are coerced to plain numbers (a 50 GiB cap is far under
  `Number.MAX_SAFE_INTEGER`). `isUpgradeError(e)` classifies the two write-gate
  codes (`upgrade_required`, `storage_quota_exceeded`).
- **`src/lib/billing.ts`**: a small per-slug status **cache** (a module `Map`) so
  the `PlanBanner` mounted on the workspace page and both game pages shares **one**
  `GET /billing` per slug per session, plus pure presentation helpers
  (`planLabel`, `statusLabel`, `intervalLabel`, `daysUntil`, and a `meter()` that
  drives the usage bars — amber at ≥ 80%, red at 100%).
- **`PlanBanner.svelte`** (self-fetching, `{slug}`): renders **nothing** when
  billing is disabled; a prominent banner on **free** (no active plan, writes
  disabled — "Choose a plan →"); a warning banner on **past_due** (grace period);
  a tiny plan **chip** on a healthy Solo/Team plan. A failed fetch renders
  nothing — it never breaks the page.
- **`/w/[slug]/billing`**: the current plan (label, Stripe status, interval,
  renewal date), a usage section (members, storage via `humanSize`,
  active share links vs limits, `∞` when unlimited), and an upgrade section with
  Solo/Team cards, a per-card Monthly/Yearly toggle, and exact pricing
  (yearly = 2 months free; tax collected at checkout by Stripe). The Subscribe
  button POSTs checkout and does a **full navigation** to `checkout_url`;
  non-owners see the cards with the button disabled. Stripe's success redirect
  (`?upgraded=1`) shows a green toast, refetches fresh status, and strips the
  param. `enabled: false` collapses the page to a single calm "self-hosted runs
  unlimited" card. Reached from a **Billing** link in the workspace header and
  from every `PlanBanner` Upgrade link.
- **Write-gate surfacing**: `push.ts` maps `upgrade_required` /
  `storage_quota_exceeded` to friendly copy, and `MathPushPanel` pairs it with an
  inline **`UpgradeNotice`** (message + "Upgrade →" link to the billing page)
  instead of a bare error. The invite-accept page gives a clear "ask the owner to
  upgrade" message when a workspace is at its member cap (the invitee has no
  billing access, so no dead link). The **share-create** flow (see below) does the
  same: a 403 `upgrade_required` on the active-link quota renders `UpgradeNotice`.

## Share links & front bundles (M5)

A game can be handed to a tester as a hosted, playable **share link**
(`<slug>.play.<domain>`) — no install, just a URL — and the spins that come back
are surfaced as analytics. The whole surface is a **Share** section on the game
page (`/w/[slug]/g/[game]`), in **`src/lib/components/SharePanel.svelte`**, with
three stacked pieces:

- **Game front** card: a share serves the game's front build (the web bundle
  players load). This card is a **read-only status probe** (HEAD on the front
  route → "bundle uploaded" / "none yet") that points back to the Revisions tab.
  The bundle itself is pushed from the unified **`PushPanel`** there: dropping an
  `index.html` folder is detected as a front bundle and runs **`runFrontPush`**
  with a compact inline progress recap (phase line + global bar; no per-file table
  — a web build is many small files). On success it toasts and notes that **new
  shares use the latest bundle automatically** (there is no list-bundles endpoint,
  so nothing is fetched back).
- **Create share** (owner/admin only — hidden for members): pin a revision
  (**Latest** tracks head, or any revision number, reusing the page's already-
  loaded revisions list) plus optional custom **slug**
  (validated `^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$`, or blank for a generated one),
  **password**, **expiry** (days), and **max concurrent sessions** (default 25).
  On success the new link is prepended to the list and its URL shown in a
  prominent copy callout. A 403 `upgrade_required` renders `UpgradeNotice`.
- **Share links list**: each `ShareLinkView` as a card — the full URL as a
  **`CopyField`** (or a "no play domain configured on this instance" hint when
  `url` is null), status/rev/bundle/`🔒 password` badges, counters (sessions,
  spins, observed RTP %, active now), expiry + session cap, and **Revoke**
  (confirm) / **Delete** (confirm) actions. Members can view and copy links but
  see no manage controls. Refresh is **manual** (a button) — nothing polls.

**Pipeline reuse** (`src/lib/push.ts`): front bundles content-address exactly
like math, so the hash → check → upload → commit orchestration is shared. The
common steps live in a private **`runPipeline`** (streaming hash, upload
planning, the bounded upload pool, and the 409 `missing_blobs` re-upload-and-
retry-once); `runPush` and **`runFrontPush`** are thin wrappers that inject only
the two endpoint-specific steps — `check` and `commit`. Bundle blobs upload
through the **same** `PUT …/blobs/:hash` (`api.games.putBlob`) as math. `runPush`
keeps its exact signature and `PushResult` shape (no caller change).

**API** (`src/lib/api.ts`): `api.shares.{list, create, update, revoke, remove}`
(revoke is a convenience `update {revoked:true}`; `update` forwards a partial
patch whose absent-vs-`null` keys carry the server's tri-state semantics, since
`JSON.stringify` drops `undefined`) and `api.games.frontCheck` / `frontCommit`.
Wire shapes go through `normalize*` helpers (`normalizeShareLink` coerces the
`bigint` counters to plain numbers) like the rest of the surface, reconciled
against the generated `crates/protocol` bindings (`ShareLinkView`,
`CreateShareRequest`, `UpdateShareRequest`, `CreateFrontBundleRequest`,
`FrontBundleCreated`, `ShareLinksResponse`). `isValidShareSlug` mirrors the
server's subdomain-label rule.

## Math report (M8)

A **Stake-Engine-style compliance report** per revision — the 2★/3★ bet-level
verdicts, per-mode metrics, and the hit-rate distribution — at
`/w/[slug]/g/[game]/r/[number]/math`. It is reached from a **Math report** button
in the revision page header (next to _Open test view_) and is auth-guarded by the
root layout like every sibling route.

The page re-uses the revision detail endpoint (`api.games.revision`) and reads the
new **`stats.analysis`** object (`RevisionAnalysis`). While stats are `pending` it
**polls every 3s** (an `$effect` teardown stops on `ok`/`error` and on unmount), so
the report materialises the moment the analyzer finishes.

The page **guides** rather than dumps: it leads with the verdict, surfaces the
problems, and hides depth behind one workspace. Its shape, top to bottom:

1. **Header** — game · rev · timestamp (just the title line).
2. **Verdict** (`MathVerdict.svelte`) — the two honest star badges (2★ / 3★
   **Within / Over limits**, plus the "estimate — Stake Engine decides" note), a
   **one-line summary per star** ("All 11 constraints within 2★ limits" or "2
   over 3★ limits: Max exposure, ETL 40x"), and — **only when something fails** —
   a compact red callout listing exactly the failing `(constraint, star)` pairs
   with their `value → limit`, each an anchor to its row (`#c-<key>`). Closes with
   the cross-mode RTP consistency line. When everything passes, the two green
   summary lines are the whole verdict (no callout).
3. **Sticky mini-nav** — a slim `sticky top-14` bar (backdrop-blur, `border-b`,
   sits under the app header, `z` below it) with **Constraints · Modes** anchors
   and the 2★ / 3★ verdict repeated as tiny pills so it never scrolls away.
4. **Bet-level compliance** (`MathConstraints.svelte`) — one decluttered line per
   `ConstraintRow`: **label · value · two limit gauges**. Helper copy ("what it
   is") moves off the row behind a per-row **ⓘ toggle** (also a `title=` tooltip).
   Single-value metrics show the value once by the label and a compact **2★ / 3★
   limit gauge** each (mini bar, amber ≥ 80%, red over the cap or on a fail, pass
   tint); per-reference-bet metrics (`max_exposure`, `max_bet_cost`) show a value
   **inside each gauge**, captioned "at &lt;ref&gt; bet". **Failing rows sort to the
   top** (stable within pass/fail groups) and each carries `id="c-<key>"`. The
   reference max bets and the failing-first note sit in the section header.
5. **Game modes** (`MathModePanel.svelte`) — the mode cards stay as the
   **selector** (name, `cost`× badge, volatility badge low=sky/medium=amber/high=red,
   Compliant/Issues, RTP / Hit / Max / B-E quartet; the active card is lit).
   Everything that follows is pinned into **one card** titled "&lt;mode&gt; —
   detailed analysis" (mode name + badges stay in the header) with internal
   **Tabs** (`Tabs.svelte`): **Metrics** (stat tiles + stacked Dead/Sub-bet/Win
   outcome bar + four streak tiles), **Compliance** (the ✓/✗ checklist), and
   **Distribution** (the hit-rate table, with a header note that row shading =
   share of RTP). The component owns only the active tab, so switching modes keeps
   the tab and switching tabs keeps the mode; the parent owns the selected mode
   (it defaults to the cost-1 base mode and resets on revision navigation).

**Defensive by construction.** Every numeric analysis field is typed
`number | null` and normalized through `numOrNull` (`normalizeRevisionAnalysis`
and friends in `api.ts`), so a partial payload from the analyzer never throws and
any missing figure renders as an em-dash rather than a misleading zero. When
`stats.analysis` is `null` (older revisions predating the analyzer) the page shows
a calm "push a new revision to recompute" hint; `pending` shows the polling
spinner and `error` surfaces the server message. Presentation helpers live in
`format.ts`: `pct(frac, dp)`, `formatOdds` ("1 in 6.80M" / "1 in 1,470"),
`formatSpins`, `formatMetric`, `formatCount`, and `xmult` (`0.96x`). The math
components are split out under `src/lib/components/Math*` (`MathVerdict`,
`MathConstraints`, `MathModePanel`) and the page owns only load/poll/selection.

## Admin console (instance operators)

An instance-operator console at **`/admin`** — global stats, workspace plan
comps, user admin management, and cross-workspace share moderation. Every
`/api/admin/*` endpoint is cookie-auth **and** admin-gated: a non-admin gets a
flat **404** on all of them, including `/me`, so gating is a boolean, never an
error path.

- **API** (`src/lib/api.ts`, `api.admin.*`): `me()` (probe → `boolean`; 404 →
  `false` without throwing), `overview()`, `workspaces(query?)`,
  `setOverride(id, {plan, expires_in_days?, note?})` (plan `null` clears the
  comp), `users(query?)`, `setAdmin(id, is_admin)` (→ `boolean`; a 409
  `last_admin` surfaces as an `ApiError` the page classifies by `code`),
  `shares(query?)`, and `revokeShare(id)`. Wire shapes go through `normalize*`
  helpers (`normalizeAdminOverview/Workspace/User/Share`) like the rest of the
  surface — `bigint` counters coerced to plain numbers, every nullable field
  tolerated — reconciled against the generated `crates/protocol` bindings.
- **Admin probe** (`src/lib/admin.ts`): a session-cached `isAdmin()` mirroring
  billing.ts's module-cache idiom — **one** `GET /api/admin/me` per session,
  shared by every mount. A definitive 404 (not admin) resolves `false` and is
  kept; a transient (non-404) failure is evicted so a later mount re-probes.
  `resetAdmin()` clears it on logout (wired into the account page's logout beside
  `resetWorkspaces()`).
- **Nav gating** (`Nav.svelte`): the **Admin** link renders only when the probe
  resolves `true`. An `$effect` keyed on `session.user?.id` re-probes if the
  signed-in user changes without a full reload; any failure keeps the link
  hidden. Non-admins never see the link, and a direct `/admin` visit by a
  non-admin renders the standard **not-found** `EmptyState` (the 404s are treated
  as "not found", never an error banner).
- **`Sparkline.svelte`**: a dependency-free tiny bar chart for a 30-day daily
  series — inline SVG, one accent-filled `<rect>` per day with a per-day
  `<title>` tooltip, stretched to fill width (`preserveAspectRatio="none"`).
  Renders a calm "no activity" panel when the series is empty or all-zero.
- **`/admin`** (`src/routes/admin/+page.svelte`): leads with eight **stat tiles**
  (users, workspaces, games, revisions, share links, storage via `humanSize`,
  sessions, spins) and two 30-day **sparklines** (signups, pushes, each with a
  running total). Below, deep-linkable `#hash` **Tabs** (reusing `Tabs.svelte`):
  - **Workspaces** — debounced (300 ms) slug/name search; a table (slug/name,
    created via `Time`, members, games, storage, plan badge — free=danger,
    solo/team=accent, unlimited=info — subscription-status text,
    and a `comped: <plan> → <date>` indicator when an override is active). Row
    action **Manage plan** expands an inline **Plan override (comp)** panel: plan
    select (None/Solo/Team/Unlimited — "None" clears), optional expiry days
    (seeded from the override's remaining days via `daysUntil`), and an optional
    note → `PUT` → toast + in-place row refresh.
  - **Users** — debounced email/name search; a table (email, display name,
    created, workspaces count, admin badge, a `you` badge on self). Action toggles
    **Make admin** / **Remove admin** (confirm on remove); a 409 `last_admin`
    surfaces inline via `toast.error`.
  - **Shares** — debounced search; a table (slug + full URL as a `CopyField` when
    present, workspace, game, sessions/spins, created, Active/Revoked status).
    Action **Revoke** (confirm) → `toast` + in-place status flip.

  Each tab loads lazily on first activation, keeps its own search/loading/error
  state, and shows a `Skeleton` while loading and a helpful `EmptyState`
  (search-aware copy) when empty. All tables sit in `overflow-x-auto` and the tile
  grid collapses (2 → 3 → 4 columns). A 404 from any admin fetch (e.g. admin
  revoked mid-session) flips the page to the not-found state rather than an error.
