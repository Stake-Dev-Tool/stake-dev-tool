-- M3: workspace document sync (profiles, saved rounds). One generic table;
-- typed kinds live in `data` (JSONB), validated per kind at the API layer.
-- `revision` is a per-document optimistic counter bumped on every write; `seq`
-- is a per-workspace global change cursor (BIGSERIAL) that clients page through
-- with `?since_seq=`. Deletes are tombstones (`deleted_at` set, row kept) so a
-- sync pull can propagate removals to other clients.

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
