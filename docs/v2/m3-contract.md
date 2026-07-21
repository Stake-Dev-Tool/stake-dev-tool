# M3 contract — document sync, workspace SSE, desktop cutover

Design decisions for M3, resolving the blockers listed in
[recon-m3-m6.md](recon-m3-m6.md) §A.3. Companion to [V2.md](../../V2.md).

## Decisions (previously open questions)

1. **Bookmarks ARE saved rounds.** One document kind (`saved_round`), no
   second concept. The test view's ★ writes a saved round, as in V1.
2. **Profile → game/revision linkage is loose.** A profile document keeps
   `game_slug` (the RGS route name) and gains optional `game` (a workspace
   game slug from M2) and `revision` (`number | null` = latest). Loose
   references: a profile may point at a game that has no cloud revisions
   yet; the UI degrades gracefully.
3. **`gamePath` (and any per-machine field) never syncs.** The desktop keeps
   a local sidecar (existing `profiles.json` becomes the overlay store,
   keyed by profile id) merged over the cloud document at read time. Synced
   profile documents contain no filesystem paths.
4. **Offline story (M3 scope):** read cache + queued writes are OUT of
   scope; the desktop requires connectivity for team features (same as the
   GitHub implementation today, which also required network). A local
   read-cache lands with M3 only if trivial; queued offline writes are a
   post-V2 enhancement.
5. **Delete-workspace exists** (owner-only) — closes the V1 parity gap.

## Document model (server)

One generic table; typed kinds live in `data` (JSONB), validated per kind.

```sql
CREATE TABLE documents (
    workspace_id UUID NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('profile', 'saved_round')),
    doc_id       TEXT NOT NULL,          -- client-generated id (uuid string)
    data         JSONB NOT NULL,
    revision     INTEGER NOT NULL DEFAULT 1,   -- per-document optimistic counter
    seq          BIGSERIAL,                    -- global change cursor (indexed)
    updated_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at   TIMESTAMPTZ,                  -- tombstone (kept for sync)
    PRIMARY KEY (workspace_id, kind, doc_id)
);
CREATE INDEX documents_seq_idx ON documents (workspace_id, seq);
```

Concurrency = optimistic with surfaced conflicts (V2.md's "LWW + conflict
surfacing" made precise):

- `PUT` carries `base_revision` (`null` = create). Server applies iff
  `base_revision == current revision` → `revision + 1`, bumps `seq`.
- Mismatch → **409 `document_conflict`** with the current server document in
  the response. The client surfaces it (dialog: keep mine / take theirs);
  "keep mine" retries with the fresh `base_revision` (client-driven LWW).
- `DELETE` follows the same rule and writes a tombstone.

### Endpoints

```
GET    /api/workspaces/:slug/documents?kind=&since_seq=   → { documents: [DocumentEnvelope], latest_seq }
GET    /api/workspaces/:slug/documents/:kind/:doc_id      → DocumentEnvelope
PUT    /api/workspaces/:slug/documents/:kind/:doc_id      { data, base_revision } → { revision, seq } | 409
DELETE /api/workspaces/:slug/documents/:kind/:doc_id      { base_revision } → { seq } | 409
DELETE /api/workspaces/:slug                              (owner) → 204; async blob cleanup
```

`DocumentEnvelope = { kind, doc_id, data, revision, seq, updated_by_display,
updated_at, deleted: bool }`. List includes tombstones when `since_seq` is
given (sync pull); excludes them otherwise. Membership required; writes
need scope `full` (PATs with only `push:math` cannot edit documents).

### Kind payloads (validated server-side, ts-rs exported)

- `profile`: `{ name, game_slug, game?: string|null, revision?: number|null,
  front_url?: string|null, resolutions: [...], created_at }` — mirrors V1
  `Profile` minus `gamePath`/`teamId`.
- `saved_round`: `{ game_slug, mode, event_id, description, revision?: number|null,
  created_at }` — V1 `SavedRound` plus the optional M2 revision pin
  (legacy imports leave it null = "latest at the time").

## Workspace SSE

```
GET /api/workspaces/:slug/events            (cookie or Bearer; membership)
```

`text/event-stream`, events:

- `document` — `{ kind, doc_id, seq }` (data NOT inlined; client pulls)
- `revision_pushed` — `{ game, number }` (hooked into the M2 commit path)
- `membership` — `{ user_id, change: "joined" | "left" | "role" }`

Implementation: `DashMap<workspace_id, tokio::sync::broadcast::Sender>` in
AppState; senders created lazily, dropped when receiver count hits zero.
Reconnect protocol: clients do NOT rely on Last-Event-ID replay — on (re)
connect they pull `?since_seq=<last known>` then stream. Keep-alive
comment every 25s. This is deliberately the same SSE idiom the LGS test
view already uses.

## Desktop cutover (M3 proper)

Per recon §A.5/A.6, on top of the already-built `cloud/` plumbing:

1. `cloud/api.rs` gains documents + SSE subscribe + M2 math push/pull
   (manifest negotiation, blob upload/download reusing the `sdt` flow).
2. `teams.rs` command bodies re-point to CloudClient; command names stay
   (`teams_*`) so `ui/` churn stays minimal. `teams.json` becomes a cache of
   `list_workspaces`.
3. `math_sync.rs` re-points Release-asset chunks → M2 blobs; keeps the
   `math-sync-progress` Tauri event contract.
4. Migration: `teams_migrate_to_cloud(team_id)` — create workspace, import
   profiles (strip `gamePath` → sidecar), saved rounds (revision = null),
   math via existing GitHub pull → M2 push as rev 1; stamp the local team
   `migratedTo`; deprecation banner in the Teams screen.
5. GitHub teams code stays but is marked deprecated (removal post-V2).

**M3 "done when" (unchanged from V2.md):** two desktop clients see each
other's saved rounds live — via document PUT + SSE `document` event.
