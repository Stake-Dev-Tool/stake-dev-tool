# Polar billing — the last mile (owner runbook)

Everything code-side is live and dormant: the server verifies webhooks,
resolves plans and enforces quotas, and the dashboard shows trial
banners, usage meters and upgrade cards — the moment the six Polar
values land in the VPS `.env.prod`. This is the exact walkthrough.
Budget: ~15 minutes of clicking + Polar's review time.

## 0. Sandbox first (recommended)

Do a dry run on https://sandbox.polar.sh (fake money, instant approval):
same steps as below with `POLAR_SERVER=sandbox`, test a checkout with
Polar's test card, watch the workspace flip from Trial to Solo, then
redo on production and overwrite the env values.

## 1. Organization + KYC (production)

1. https://polar.sh → sign in → create the organization (e.g.
   "Stake Dev Tool").
2. Fill the org profile: website `https://stakedevtool.com`, support
   email `support@stakedevtool.com` (Cloudflare Email Routing already
   forwards it).
3. Start identity/payout verification (Stripe-powered KYC: identity,
   SIRET/company details, bank account). **Positioning tip**: this is
   B2B developer tooling — a test bench for slot-game developers on the
   Stake Engine RGS contract. It is not gambling and no wagering happens
   on the platform; say exactly that if compliance asks.

## 2. The four products

Polar products carry ONE billing cycle each → create four (Products →
New). Suggested (from V2.md pricing — adjust freely, the server only
cares about the IDs):

| Product name | Price | Cycle |
|---|---|---|
| Solo — Monthly | €5 | monthly |
| Solo — Yearly | €48 | yearly |
| Team — Monthly | €15 | monthly |
| Team — Yearly | €144 | yearly |

No Polar "benefits" needed — entitlements are enforced by our server
via the webhook. Copy each product's ID (Products → click product → ID).

## 3. Credentials

- **Access token**: Settings → Developers → New token (scopes:
  checkouts + products read is enough; org-scoped).
- **Webhook**: Settings → Webhooks → Add endpoint →
  URL `https://app.stakedevtool.com/api/billing/webhook`, format RAW —
  select the `subscription.*` and `order.created` events → copy the
  signing secret (`whsec_…`).

## 4. Flip the switch on the VPS

```bash
ssh root@159.195.158.20
cd /opt/stake-dev-tool/deploy && sh enable-billing.sh
```

The script prompts for the six values, restarts the server, and billing
goes live: every existing workspace starts a 14-day trial from its
creation date (old workspaces may land directly in "expired" — that's
the designed read-only nudge; upgrading unlocks instantly).

## 5. Verify (two minutes)

1. Dashboard → any workspace → **Billing**: trial state + upgrade cards
   visible (`enabled` is now true).
2. Click an upgrade → Polar checkout opens → pay (sandbox: test card
   4242…) → you land back on the workspace with the success toast.
3. The workspace shows the plan chip; the Polar dashboard shows the
   subscription; `billing_events` on the box records the webhook.

## Rollback

Comment the `POLAR_*` lines back out of `.env.prod`, `docker compose up
-d server` — the instance returns to unlimited, nobody loses data.
