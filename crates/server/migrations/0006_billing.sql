-- M7: Polar billing. One subscription per workspace (keyed on workspace so the
-- plan resolution is a single-row lookup), plus an append-only ledger of raw
-- webhook events. The event `id` (Standard-Webhooks message id) is the PK, which
-- makes processing idempotent under Polar's at-least-once retries.

CREATE TABLE subscriptions (
    workspace_id          UUID PRIMARY KEY REFERENCES workspaces (id) ON DELETE CASCADE,
    polar_subscription_id TEXT NOT NULL UNIQUE,
    polar_customer_id     TEXT,
    plan                  TEXT NOT NULL CHECK (plan IN ('solo', 'team')),
    "interval"            TEXT NOT NULL CHECK ("interval" IN ('monthly', 'yearly')),
    status                TEXT NOT NULL,          -- Polar's status verbatim
    current_period_end    TIMESTAMPTZ,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Raw webhook ledger: every accepted (signature-valid) event is inserted here
-- before processing. `processed_at` marks a successfully handled event;
-- `error` records why an authentic-but-unprocessable event (unknown workspace,
-- malformed body) was skipped without a 5xx retry storm.
CREATE TABLE billing_events (
    id           TEXT PRIMARY KEY,       -- Standard-Webhooks `webhook-id`
    type         TEXT NOT NULL,
    payload      JSONB NOT NULL,
    received_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    error        TEXT
);
