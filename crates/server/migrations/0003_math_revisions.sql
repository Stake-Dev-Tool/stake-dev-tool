-- M2: content-addressed math revisions. A game holds immutable, numbered
-- revisions; each revision is a manifest of (path -> blob hash) plus the
-- per-mode bet stats computed from its lookup tables. Blob *bytes* live in the
-- object store (key `blobs/<workspace>/<hex sha256>`); only the sha256 hash and
-- size are tracked here.

CREATE TABLE games (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    -- Same rule as workspace slugs: ^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$
    slug         TEXT NOT NULL,
    name         TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, slug)
);

CREATE INDEX games_workspace_id_idx ON games (workspace_id);

-- Content-addressed blobs. Dedup is scoped PER WORKSPACE on purpose. A global
-- (hash-only) blobs table would let one tenant "claim" another tenant's blob by
-- presenting only its hash — without ever possessing the bytes — turning the
-- store into a cross-tenant existence oracle (a classic dedup attack). Scoping
-- by workspace forces every tenant to actually upload the bytes into its own
-- namespace before a revision can reference them.
CREATE TABLE blobs (
    workspace_id UUID NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    hash         BYTEA NOT NULL,   -- sha256, 32 bytes
    size         BIGINT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, hash)
);

CREATE TABLE revisions (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    game_id    UUID NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    number     INTEGER NOT NULL,
    message    TEXT NOT NULL,
    -- Kept for provenance; NULL if the author's account is later deleted.
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (game_id, number)
);

CREATE INDEX revisions_game_id_idx ON revisions (game_id);

CREATE TABLE revision_files (
    revision_id UUID NOT NULL REFERENCES revisions (id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    hash        BYTEA NOT NULL,
    size        BIGINT NOT NULL,
    PRIMARY KEY (revision_id, path)
);

-- One stats row per revision, filled asynchronously after the commit. `data`
-- holds `{"modes":[{mode,cost,rtp,max_win,entries,hit_rate}, ...]}` when status
-- is 'ok'; `error` carries the failure message when status is 'error'.
CREATE TABLE revision_stats (
    revision_id UUID PRIMARY KEY REFERENCES revisions (id) ON DELETE CASCADE,
    status      TEXT NOT NULL CHECK (status IN ('pending', 'ok', 'error')),
    error       TEXT,
    data        JSONB,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
