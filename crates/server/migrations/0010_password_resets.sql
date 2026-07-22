-- Password reset, Discord OAuth identities, and signup email verification.
-- Token tables mirror the identity model from 0002: the raw secret is never
-- stored, only its sha256 (token_hash), so lookups are by hash.

-- One-shot password reset tokens (1h TTL, single use).
CREATE TABLE password_resets (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ
);

CREATE INDEX password_resets_user_id_idx ON password_resets (user_id);

-- Discord OAuth links. discord_id is TEXT: snowflakes exceed a 32-bit int and
-- TEXT sidesteps any signed/unsigned ambiguity (mirrors github_identities).
CREATE TABLE discord_identities (
    discord_id TEXT PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    username   TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX discord_identities_user_id_idx ON discord_identities (user_id);

-- Email verification. Existing accounts predate the system, so backfill them as
-- verified — the feature only gates NEW signups on instances with mail set up.
ALTER TABLE users ADD COLUMN email_verified_at TIMESTAMPTZ;
UPDATE users SET email_verified_at = now();

-- One-shot email verification tokens (24h TTL, single use).
CREATE TABLE email_verifications (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ
);

CREATE INDEX email_verifications_user_id_idx ON email_verifications (user_id);
