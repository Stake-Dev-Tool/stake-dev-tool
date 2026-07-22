#!/bin/sh
# Enable Stripe billing on this instance. Run ON the server:
#   cd /opt/stake-dev-tool/deploy && sh enable-billing.sh
# It prompts for the Stripe values, writes them into .env.prod, restarts
# the server, and verifies that billing reports enabled.
# Self-hosters: skip this entirely — without STRIPE_* vars the instance
# runs unlimited, forever.
set -e

ENVF=".env.prod"
[ -f "$ENVF" ] || { echo "run me from /opt/stake-dev-tool/deploy (no $ENVF here)"; exit 1; }

prompt() {
    # $1 = env key, $2 = label
    printf "%s: " "$2"
    read -r value
    [ -n "$value" ] || { echo "empty value, aborting (nothing written)"; exit 1; }
    if grep -q "^$1=" "$ENVF"; then
        sed -i "s|^$1=.*|$1=$value|" "$ENVF"
    elif grep -q "^# $1=" "$ENVF"; then
        sed -i "s|^# $1=.*|$1=$value|" "$ENVF"
    else
        echo "$1=$value" >> "$ENVF"
    fi
}

prompt STRIPE_SECRET_KEY         "Secret key (sk_live_… or sk_test_…)"
prompt STRIPE_WEBHOOK_SECRET     "Webhook signing secret (whsec_…)"
prompt STRIPE_PRICE_SEAT_MONTHLY "Price ID — per-seat monthly, graduated tiers €3/€2 (price_…)"
prompt STRIPE_PRICE_SEAT_YEARLY  "Price ID — per-seat yearly, graduated tiers €30/€20"
prompt STRIPE_PRICE_STORAGE      "Price ID — extra storage (per 10 GiB unit)"

echo "Restarting the server with billing enabled…"
docker compose -f docker-compose.prod.yml --env-file "$ENVF" up -d server >/dev/null

echo "Waiting for health…"
i=0
while [ $i -lt 30 ]; do
    if docker compose -f docker-compose.prod.yml --env-file "$ENVF" exec -T server \
        curl -fsS http://127.0.0.1:8080/healthz >/dev/null 2>&1; then
        break
    fi
    i=$((i + 1)); sleep 2
done

echo
echo "Done. Verify from your machine:"
echo "  1. log into the dashboard, open any workspace -> Billing"
echo "  2. the page should show the No-plan state and the plan cards"
echo "Then point the Stripe webhook (dashboard -> Developers -> Webhooks) at:"
echo "  https://app.stakedevtool.com/api/billing/webhook"
