#!/bin/sh
# Enable Polar billing on this instance. Run ON the server:
#   cd /opt/stake-dev-tool/deploy && sh enable-billing.sh
# It prompts for the six Polar values, writes them into .env.prod, restarts
# the server, and verifies that billing reports enabled.
# Full walkthrough: docs/v2/polar-runbook.md
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

echo "Polar environment (production or sandbox)?"
printf "POLAR_SERVER [production]: "
read -r server
server="${server:-production}"
if grep -q "^POLAR_SERVER=" "$ENVF"; then
    sed -i "s|^POLAR_SERVER=.*|POLAR_SERVER=$server|" "$ENVF"
else
    echo "POLAR_SERVER=$server" >> "$ENVF"
fi

prompt POLAR_ACCESS_TOKEN        "Organization access token (polar_oat_…)"
prompt POLAR_WEBHOOK_SECRET      "Webhook signing secret (whsec_… or raw)"
prompt POLAR_PRODUCT_SOLO_MONTHLY "Product ID — Solo monthly"
prompt POLAR_PRODUCT_SOLO_YEARLY  "Product ID — Solo yearly"
prompt POLAR_PRODUCT_TEAM_MONTHLY "Product ID — Team monthly"
prompt POLAR_PRODUCT_TEAM_YEARLY  "Product ID — Team yearly"

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
echo "Done. Verify from your machine (expects \"enabled\":true for members):"
echo "  1. log into the dashboard, open any workspace -> Billing"
echo "  2. the page should show the Trial state and the upgrade cards"
echo "Then point the Polar webhook at: https://app.stakedevtool.com/api/billing/webhook"
