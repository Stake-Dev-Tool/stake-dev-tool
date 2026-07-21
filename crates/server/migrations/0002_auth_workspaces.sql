-- M1: identity (users, sessions, API tokens, device codes, GitHub links) and
-- workspaces (memberships, invites). gen_random_uuid() is built into Postgres
-- 13+ core, so no pgcrypto/uuid-ossp extension is required.

CREATE TABLE users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT NOT NULL,
    -- NULL for accounts that only sign in via GitHub (no usable password).
    password_hash TEXT,
    display_name  TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Case-insensitive email uniqueness without the citext extension.
CREATE UNIQUE INDEX users_email_lower_key ON users (lower(email));

CREATE TABLE github_identities (
    github_id  BIGINT PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    login      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX github_identities_user_id_idx ON github_identities (user_id);

-- Bearer secrets are never stored; token_hash is sha256(full secret string).
CREATE TABLE sessions (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash   BYTEA NOT NULL UNIQUE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ
);

CREATE INDEX sessions_user_id_idx ON sessions (user_id);

CREATE TABLE api_tokens (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   BYTEA NOT NULL UNIQUE,
    scopes       TEXT[] NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX api_tokens_user_id_idx ON api_tokens (user_id);

CREATE TABLE device_codes (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    device_code_hash BYTEA NOT NULL UNIQUE,
    user_code        TEXT NOT NULL UNIQUE,
    -- Set when a signed-in user approves the pairing.
    user_id          UUID REFERENCES users (id) ON DELETE CASCADE,
    approved         BOOLEAN NOT NULL DEFAULT false,
    denied           BOOLEAN NOT NULL DEFAULT false,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at       TIMESTAMPTZ NOT NULL,
    last_polled_at   TIMESTAMPTZ
);

CREATE TABLE workspaces (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug       TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    -- Kept for provenance; NULL if the creator's account is later deleted.
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE memberships (
    workspace_id UUID NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role         TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id)
);

-- Composite PK indexes workspace_id first; this covers "which workspaces does
-- this user belong to?".
CREATE INDEX memberships_user_id_idx ON memberships (user_id);

CREATE TABLE invites (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    token_hash   BYTEA NOT NULL UNIQUE,
    role         TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    created_by   UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '7 days',
    -- 0 means unlimited.
    max_uses     INTEGER NOT NULL DEFAULT 0,
    uses         INTEGER NOT NULL DEFAULT 0,
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX invites_workspace_id_idx ON invites (workspace_id);
