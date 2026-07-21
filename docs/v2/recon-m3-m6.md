# V2 recon — Teams sync (M3) & test view (M6)

Read-only reconnaissance of the V1 code that milestones M3 and M6 replace or
factor. File:line references are against branch `v2` at the time of writing
(2026-07-21). Companion to [V2.md](../../V2.md).

---

## Part A — "Teams" (GitHub-repo) sync → cloud (M3)

The V1 team system spans **`crates/desktop`** (all GitHub logic),
**`crates/lgs`** (owns the `SavedRound` type + local store) and **`ui/`**
(screens). The desktop crate has **no `protocol` dependency and no cloud
client today** (`reqwest` + `keyring` are GitHub-only).

### A.1 Synced data kinds

| Kind | Local on-disk | Remote wire (team repo) | Notes |
|---|---|---|---|
| Teams registry | `%LOCALAPPDATA%/stake-dev-tool/teams.json` — `TeamsFile { activeTeamId, teams }` (`teams.rs:42-48`) | *not synced* | Purely local "which teams have I joined". Cloud equivalent = workspace membership from the server. |
| Team manifest | — | `.stake-team.json` — `TeamManifest { schemaVersion, teamId, teamName, createdAt }` (`teams.rs:50-60`) | Cloud equivalent = `workspaces` row. `TEAM_SCHEMA_VERSION=1`. |
| Profiles | `profiles.json` — `Profile { id, name, gamePath, gameUrl, gameSlug, resolutions[], … , teamId }` (`profiles.rs:9-32`) | `profiles/<id>.json` | On push, `gamePath` (machine-local) is **blanked** and `teamId` nulled (`teams.rs:610-613`). Catalogue + explicit pull, not bidi sync. Dead `sync_profiles` remains (`teams.rs:943-1033`). |
| Saved rounds / bookmarks | `saved-rounds.json` — `SavedRound { id, gameSlug, mode, eventId, description, … }` — owned by `lgs` (`lgs/src/saved_rounds.rs:8-22`) | `saved-rounds/<id>.json` | **Bidi LWW** on `updatedAt` (`teams.rs:1035-1153`). "Bookmarks" == saved rounds (test view ★ writes a `SavedRound`). Keyed `(gameSlug, mode, eventId)` — **no revision**. |
| Math files | `<root>/<slug>/`: `index.json` (modes), `books_*.jsonl.zst` (zstd JSONL, multi-GB), lookup CSVs (`math_engine.rs:134-148`) | Manifest `math-manifests/<slug>.json` (`MathManifest`, `math_sync.rs:70-96`) + binaries as GitHub **Release** assets tagged `math-<slug>`, chunks ≤ 1 GiB, SHA-suffixed names | Content-addressed asset names → dedup within a release. Push `math_sync.rs:359-572`, pull `:574-736`. |
| Settings / resolutions | `settings.json` (`lgs/src/settings.rs:16-19`) | *not synced* | Only the per-profile `resolutions` snapshot travels, inside the profile JSON. |

### A.2 Operations (Tauri commands in `crates/desktop/src/commands.rs`, registered `lib.rs:55-73`, wrapped by `teamsApi`/`githubAuth` in `ui/src/lib/api.ts:512-620`)

- **Lifecycle**: create `teams.rs:263` / join `:373` / leave-local `:215` / delete `:230` (`DELETE /repos`, owner-only) / invite (GH collaborator PUT, `github/api.rs:282`) / discover (topic search `github/api.rs:299`) / active-set-list `teams.rs:181-195`.
- **Catalogue**: push profile `teams.rs:591` (profile + its game's rounds) / list `:674` / all catalogs `:826` / pull `:728` (composes `math_sync::pull` + `profiles::upsert_raw` + rounds) / remove-from-catalog `:440` (owner-only).
- **Sync**: `sync_team` `teams.rs:912` → only `sync_saved_rounds` (LWW at `:1094`, `:1145`); profiles report 0/0.
- **Math**: push `math_sync.rs:359` / pull `:574` / list remote `:738`; progress via Tauri event `math-sync-progress` (`math_sync.rs:38`).
- **Auth**: GitHub device flow (`github/auth.rs:82,120,227,219`), keyring service `"stake-dev-tool"` / user `"github-oauth-token"`; OAuth scopes `repo read:user delete_repo`.
- **Conflict handling today**: LWW-by-`updatedAt` for rounds; whole-file-SHA-wins for math; profiles dodge conflicts by design. **Nothing surfaced to the user.**

### A.3 M3 coverage vs V2.md — explicit gaps

Covered by the V2 model (workspaces/invites M1, blobs+revisions M2, versioned
docs + SSE M3): create/join/leave, invites (identity model changes), discover
(→ list my workspaces), roles (superset), profiles catalogue, rounds sync
(improved), math push/pull (improved: cross-revision dedup, resumable),
in-profile resolutions, progress overlay (client-driven upload, event kept).

**Not covered — decide before building M3:**
1. **`gamePath` local-only overlay** — profiles carry a per-machine path that
   must never sync. Need a local sidecar keyed by profile id merged over the
   server doc.
2. **Profile → game → revision linkage** — V1 profiles are free-form
   `gameSlug`+`gameUrl`; V2 has workspaces→games→revisions. Schema undefined.
3. **"Bookmarks" vs "saved rounds"** — one kind today; V2.md names both.
4. **GH-org team repos** — no workspace analogue; dropped.
5. **Offline / local-first** — local JSON works offline; server-authoritative
   docs don't. No desktop offline/queue story yet.
6. **Conflict-surfacing UI** — required by V2.md, net-new.
7. **Legacy rounds have no revision** — imported rounds attach to "latest" or
   stay unpinned.
8. **Delete-workspace endpoint** — no protocol type/endpoint yet (V1 had
   delete team).

### A.4 Migration path (GitHub team → workspace)

Importable via the **existing** `GithubClient`: manifest → workspace
(name/slug), `profiles/*.json` → profile documents (drop `gamePath`),
`saved-rounds/*.json` → saved-round documents (legacy: unpinned/latest),
math manifests + Release assets → download via reuse of `math_sync::pull`
into a temp dir, then push as the workspace's **rev 1** through the M2 flow.

Dropped: GH collaborator list (re-invite via cloud invites), org link,
repo/releases/topics, local `teams.json` entry, topic discovery.

Hook: new `crates/desktop/src/cloud/migrate.rs` + Tauri command
`teams_migrate_to_cloud(team_id, workspace?)`; per-team "Migrate to cloud"
button + deprecation banner on `ui/src/routes/teams/+page.svelte`.

### A.5 Desktop seams (GitHub client → cloud client)

GitHub surface splits cleanly: **team sync** uses Contents API + Releases +
repo lifecycle; **preview/share (M5)** uses Git Data API + Pages — confined
to `preview.rs:1082-1207`, untouched by M3.

Swap points:
1. **Auth**: clone `github/auth.rs` → `cloud/auth.rs` driving the server's
   device flow (`protocol::Device*`); new keyring user (`"cloud-token"`).
2. **Client**: new `cloud/api.rs` `CloudClient` mirroring `GithubClient`
   (bearer token, base URL from config, not hardcoded). Add `protocol` dep to
   `crates/desktop`.
3. **Base URL config**: `cloud/config.rs` — hosted default + env + settings
   field so self-hosters point at their instance; expose Tauri command.
4. **Re-point logic**: `teams.rs` → document calls; `math_sync.rs` → blob
   manifest-negotiation upload (keep `math-sync-progress` event).
   `teams.json` becomes a thin cache or is dropped.
5. **Command surface**: keep `teams_*` command names as the UI contract,
   re-point bodies.

### A.6 M3 task breakdown

1. Desktop cloud plumbing (`protocol` dep, `cloud/{config,auth,api}.rs`).
2. Document sync client (versioned docs, LWW, conflict object surfaced,
   `gamePath` sidecar).
3. Workspace SSE → live rounds between two desktops (M3 done-criteria).
4. Math over cloud (re-point `math_sync.rs`).
5. Migration command + Teams-page UI.
6. Schema decisions (blockers): profile→game/revision link; bookmark
   concept; delete-workspace; legacy-round revision policy.
7. Server coordination: delete-workspace; document endpoints + SSE;
   tenant-scoped saved-round docs.

---

## Part B — Test view architecture (M6)

**Headline: the test view is already ~90 % decoupled.**
`ui/src/routes/test/+page.svelte` (1620 lines) has **zero `@tauri-apps`
imports**; it reads config from URL query params, talks to the LGS
same-origin (HTTP + SSE) and iframes the game front. M6 is mostly isolation,
not rewrite.

### B.1 Tauri coupling inventory (ui/)

| File | Tauri import | Role |
|---|---|---|
| `ui/src/lib/api.ts:1-2` | `invoke`, dialog `open`, dynamic updater/app/process | **Mixed module**: invoke clients AND the Tauri-free `*Http` fetch clients. Test view imports only the http subset, but the top-level Tauri import drags Tauri into the `/test` chunk. |
| `ui/src/routes/+page.svelte:3-4` | `openUrl`, `getCurrentWindow` | Desktop chrome (launcher). Launches external Chromium at `https://localhost:<port>/test/?gameUrl=…`. |
| `ui/src/routes/teams/+page.svelte:4` | `openUrl` | Desktop chrome. |
| `ui/src/lib/components/GithubSignInDialog.svelte:3` | `openUrl` | Desktop chrome. |
| `ui/src/lib/components/MathSyncOverlay.svelte:3` | `listen` | **In the shared root layout** (`+layout.svelte:8`) → leaks onto `/test`; tolerated because the rejected promise is swallowed in a plain browser. |

Test view ambient inputs: query params (`gameUrl`, `gameSlug`,
`test/+page.svelte:553-555`), same-origin base (`:434-435`), `*Http` clients
+ raw `prepareSession` (`:660`) on `/api/devtool/*`, SSE `EventSource`
(`:387` ↔ `lgs/src/devtool.rs:227`), `localStorage` keys (`:436-439`),
iframe with `rgs_url = <host>/api/rgs/<slug>` (`buildGameUrlFor`
`:647-658`) — the Stake front contract
(`sessionID, rgs_url, lang, currency, device, social`).

Build: adapter-static SPA embedded in the LGS via
`include_dir!(../../ui/build)` (`lgs/src/lib.rs:36-38`), disk-served in
debug.

### B.2 Factoring design — one component, three contexts

Introduce a `TestViewContext` injected at mount:

```ts
type TestViewContext = {
  apiBase: string;      // '' (desktop) | ws/game/rev-scoped prefix (cloud) | share origin
  frontUrl: string;     // localhost dev server | uploaded bundle URL
  gameSlug: string;
  makeSessionId(resId): string;
  capabilities: { manageResolutions; forceEvent; saveRounds; reset; openInBrowser };
  auth?: { workspace?; revision? };
};
```

Moves:
1. **Split `api.ts`** → `api.http.ts` (Tauri-free) / `api.tauri.ts`;
   parameterise http clients + SSE + prepare on `apiBase`. `/test` chunk
   loses Tauri entirely.
2. **Extract** the page body into `ui/src/lib/testview/TestView.svelte`
   consuming the context; context factories in
   `ui/src/lib/testview/context.ts` (`desktopContext`, `cloudContext`,
   `shareContext`).
3. `/test` becomes a thin desktop wrapper (regression baseline: zero
   behavior change).
4. **Move `MathSyncOverlay`** into a `(desktop)` route group layout so
   `/test` sits on a bare layout.
5. Generalise front-URL building; keep the Stake query-param contract
   byte-identical.

### B.3 What the cloud workbench needs beyond today

1. **Auth context** — cookie session, every `/api/devtool` + `/api/rgs`
   call scoped `(workspace, game, revision)`. Workbench pages are net-new.
2. **Selection UI** — ws → game → revision (pin/latest) upstream of the view.
3. **Front sourcing** — uploaded bundle served same-origin (default) vs
   optional localhost dev URL. Needs server bundle upload/serving (overlaps
   M5).
4. **Sessions** — desktop ids are single-user; multi-tenant needs per-user
   namespacing and authorization. ⚠ `DELETE /api/devtool/sessions` today
   calls `reset_all()` **globally** (`devtool.rs:92-95`) — must be
   tenant-scoped in M4.
5. **Resolutions** — single global `settings.json` today; make per-user
   document or client-side only.
6. **Bet-stats / modes** must resolve against the pinned revision's
   materialized math (M4); endpoint shape can stay.

### B.4 Risks / unknowns

- **Mixed content**: HTTPS workbench cannot iframe a plain-http localhost
  front. Likely product boundary: cloud workbench = uploaded bundles;
  "cloud math + local front" stays a desktop feature.
- **CORS invariant**: keep front + RGS same-origin everywhere (share does).
- **WebGL context ceiling**: desktop Chromium is launched with
  `--max-active-webgl-contexts=64` (`commands.rs:544-560`); a normal tab
  caps ~16 → the multi-resolution matrix needs lazy-mount/recycling.
- **SSE auth**: `EventSource` can't send headers → cookie (same-origin) or
  token query param; devtool SSE is unauthenticated today.
- **localStorage bleed** on a shared `app.<domain>` origin → key by
  workspace/revision too.
- **Lock in M4**: the multi-tenant LGS should expose the *same*
  `/api/devtool/*` + `/api/rgs/*` surface per tenant modulo a base prefix —
  then M6 is a thin re-base.

### B.5 M6 task breakdown

1. Split `api.ts`; parameterise on `apiBase`.
2. Extract `TestView.svelte` + context + factories; `/test` thin wrapper.
3. De-couple layout (`(desktop)` route group); verify `/test` bundle has
   zero Tauri.
4. Cloud shell: authed workbench routes + selection; `cloudContext()`.
5. Front sourcing: bundle serving + local-dev toggle.
6. Server scoping (with M4): tenant-scoped devtool routes (esp. session
   reset), per-user resolutions, revision-scoped stats.
7. WebGL graceful degradation.
