-- M5: share links on wildcard subdomains. A game gets one or more "front
-- bundles" (its playable web build, pushed exactly like math via the M2 blob
-- machinery) and any number of "share links". A share link maps a DNS label
-- (`<slug>.play.<domain>`) to a (game, revision, front bundle) instance served
-- to anonymous visitors against the real cloud LGS. Lifetime play counters live
-- on the row and are incremented atomically in the wallet path.

-- A game's front-end build, content-addressed like a math revision. The blob
-- *bytes* live in the SAME per-workspace object store (key
-- `blobs/<workspace>/<hex sha256>`), uploaded through the existing
-- `PUT /workspaces/:slug/games/:game/blobs/:hash` endpoint; only the manifest is
-- tracked here.
CREATE TABLE front_bundles (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    game_id    UUID NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    -- path -> {"hash": "<hex sha256>", "size": <bytes>}; `index.html` required at
    -- the root. Validated by the create handler before insert.
    manifest   JSONB NOT NULL,
    -- Kept for provenance; NULL if the author's account is later deleted.
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX front_bundles_game_id_idx ON front_bundles (game_id);

CREATE TABLE share_links (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id            UUID NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    game_id                 UUID NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    -- Subdomain label: ^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$ (or a single alnum).
    -- UNIQUE so a host lookup (`<slug>.play.<domain>`) resolves at most one link.
    slug                    TEXT NOT NULL UNIQUE,
    -- NULL = track the game's latest revision; otherwise pin this number.
    revision_number         INTEGER,
    -- NULL = serve the game's latest front bundle; otherwise pin this one.
    front_bundle_id         UUID REFERENCES front_bundles (id) ON DELETE SET NULL,
    -- Argon2 PHC string; NULL = public (no interstitial).
    password_hash           TEXT,
    -- NULL = never expires.
    expires_at              TIMESTAMPTZ,
    max_concurrent_sessions INTEGER NOT NULL DEFAULT 25,
    -- Non-NULL disables the link (a revoked link 404s like an unknown one).
    revoked_at              TIMESTAMPTZ,
    created_by              UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Lifetime analytics, incremented atomically in the wallet path.
    sessions_count          BIGINT NOT NULL DEFAULT 0,
    spins_count             BIGINT NOT NULL DEFAULT 0,
    total_bet               NUMERIC NOT NULL DEFAULT 0,
    total_win               NUMERIC NOT NULL DEFAULT 0
);

CREATE INDEX share_links_workspace_id_idx ON share_links (workspace_id);
CREATE INDEX share_links_game_id_idx ON share_links (game_id);
