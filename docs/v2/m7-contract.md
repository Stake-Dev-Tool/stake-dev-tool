# M7 contract — billing (Polar) in the server, plans & quotas

Companion to [V2.md](../../V2.md) M7 and the pricing section. Principle
(unchanged): billing code ships in the open-source server but only
activates on instances with Polar configured; self-hosters run with
everything unlimited and never see it.

## Activation

Like GitHub OAuth: the billing module is enabled iff `POLAR_ACCESS_TOKEN`,
`POLAR_WEBHOOK_SECRET` (and the product ids) are set. Disabled → billing
routes 404, `plan()` resolves to `Unlimited`, zero quota checks fire.

## Checkout flow (dashboard-initiated, metadata-keyed)

The site's marketing pages link to the dashboard; the dashboard starts
checkout so the workspace binding is never guessed from an email:

```
POST /api/workspaces/:slug/billing/checkout { plan: "solo"|"team", interval: "monthly"|"yearly" }
  → owner-only → server creates a Polar checkout session (metadata.workspace_id)
  → { checkout_url }  (browser redirects there)
GET  /api/workspaces/:slug/billing → { plan, status, interval, current_period_end,
                                       usage: {members, storage_bytes, share_sessions},
                                       limits: {…}, portal_url }
```

`portal_url` comes from Polar's customer portal (manage/cancel/invoices —
they are the merchant of record).

## Webhook (server-owned; the `site/` webhook route retires here)

```
POST /api/billing/webhook   (no auth; Standard-Webhooks HMAC signature
                             verified with POLAR_WEBHOOK_SECRET)
```

- Events land append-only in `billing_events (id TEXT PK, type, payload
  JSONB, received_at, processed_at, error)` — the unique id makes
  processing idempotent under Polar retries.
- Handled types: `subscription.created/updated/active/canceled/revoked`
  (+ `order.created` for audit). Everything else: stored, marked
  processed, ignored.
- Effect: upsert into `subscriptions`:

```sql
CREATE TABLE subscriptions (
    workspace_id UUID PRIMARY KEY REFERENCES workspaces (id) ON DELETE CASCADE,
    polar_subscription_id TEXT NOT NULL UNIQUE,
    polar_customer_id     TEXT,
    plan     TEXT NOT NULL CHECK (plan IN ('solo','team')),
    interval TEXT NOT NULL CHECK (interval IN ('monthly','yearly')),
    status   TEXT NOT NULL,              -- Polar's status verbatim
    current_period_end TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Workspace resolution: `metadata.workspace_id` (set at checkout). Unknown /
missing metadata → event stored with an error, surfaced in logs; never a
5xx to Polar (avoid poison-pill retries) except on signature failure.

## Plans & quotas

Limits are code constants (one place, `billing/plan.rs`):

| | Trial (14d) | Solo | Team | Unlimited (self-host / billing off) |
|---|---|---|---|---|
| members | 3 | 1 | 10 | ∞ |
| storage (blobs, math+front) | 2 GiB | 10 GiB | 50 GiB | ∞ |
| concurrent share sessions / link | 5 | 5 | 25 | ∞ |
| active share links | 2 | 5 | 25 | ∞ |

- `plan(workspace)` resolves: billing disabled → Unlimited; else
  subscription active/trialing → its plan; past_due → plan with a 7-day
  grace (then Trial-like read-only); none → Trial from workspace
  `created_at` (expired trial → read-only: pulls and dashboard work,
  pushes/new shares/invites blocked with `upgrade_required`).
- Enforcement points (server-side, single helper): invite create/accept
  (member count), blob upload + revision/front-bundle commit (per-workspace
  stored-bytes sum, tracked on `blobs`), share link create (active count),
  visitor session create (concurrent per link — already in M5's model).
- Storage accounting: `SUM(size)` over the workspace's `blobs` rows
  (dedup'd bytes — generous in the user's favor, cheap to compute,
  cached in-process for 60s).

## Out of scope for the server

Tax/VAT/invoices/refunds: Polar (MoR). Seat-based pricing, overage
add-ons, currency display: post-V2. The `site/` keeps marketing +
pricing pages; its checkout/webhook routes become thin redirects to the
dashboard flow (owner's cleanup, not blocking).
