-- Short-lived capability tokens for the cloud workbench's cross-origin mount.
--
-- The workbench normally serves the game front same-origin, so the session
-- cookie authorizes the tenant LGS mount under /api/ws/…. A front running on the
-- developer's own dev server is a different SITE, and the SameSite=Lax session
-- cookie is never attached to its cross-site calls. Such a front reaches the LGS
-- through /api/wb/<token>/… instead: the token carries the entire authorization
-- (who, and which workspace/game/revision) in the PATH, so no ambient credential
-- is involved and the CORS layer on that mount needs no credentials at all.
--
-- Same token rule as every other credential here: the raw secret is never
-- stored, only its sha256.
CREATE TABLE workbench_tokens (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash   BYTEA NOT NULL UNIQUE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    game_id      UUID NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    revision_id  UUID NOT NULL REFERENCES revisions (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL
);

CREATE INDEX workbench_tokens_user_id_idx ON workbench_tokens (user_id);
-- Expired rows are swept opportunistically on mint; the index keeps that cheap.
CREATE INDEX workbench_tokens_expires_at_idx ON workbench_tokens (expires_at);
