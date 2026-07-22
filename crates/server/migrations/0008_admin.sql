-- Instance administration (platform operators). A per-user `is_admin` flag plus
-- a table of comp/manual plan overrides. Admin identity is EITHER this flag OR
-- membership in the `SERVER_ADMIN_EMAILS` config list (the env list bootstraps
-- the first admin without any SQL). A `plan_overrides` row grants a workspace a
-- plan for free (a "comp subscription"); it is consulted by `plan_for` before
-- the Polar subscription resolution and ignored once `expires_at` has passed.

ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE plan_overrides (workspace_id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE, plan TEXT NOT NULL CHECK (plan IN ('solo','team','unlimited')), expires_at TIMESTAMPTZ, note TEXT, created_by UUID REFERENCES users(id) ON DELETE SET NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
