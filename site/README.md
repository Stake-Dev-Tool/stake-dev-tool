# site

Marketing site for Stake Dev Tool, built with
[TanStack Start](https://tanstack.com/start), React and Tailwind CSS v4.

## Develop

```bash
pnpm site:dev      # from the repo root, serves on http://localhost:3000
```

## Build

```bash
pnpm site:build    # outputs a Nitro node server in site/.output
node site/.output/server/index.mjs
```

The Nitro preset can be switched (static hosting, Cloudflare, etc.) via
`nitro` options in `vite.config.ts` once deployment is decided.

## Structure

- `src/routes/`: one file per page — `index` (home), `features`, `cloud`,
  `pricing`, `open-source`, plus `checkout.success` / `checkout.cancelled`
  and the `api.billing.webhook` server route.
- `src/components/TestViewFigure.tsx`: the hero figure. The same mini slot
  front at three resolutions inside one app window, with a live SSE event
  ticker.
- `src/styles.css`: design tokens (spruce/mint/amber palette, Bricolage
  Grotesque + Geist type) and the few custom CSS pieces (ticker, frames,
  section rules).

Content mirrors `V2.md`. Keep pricing and cloud copy in sync with it.

## Billing

Payments go through Polar (polar.sh) as merchant of record (they remit EU
VAT for us — see V2.md).

- `src/server/billing.ts`: `createCheckout` server function. Creates a
  hosted Polar checkout for a plan/interval and returns its URL.
- `src/routes/api.billing.webhook.ts`: webhook receiver; signatures are
  verified with the Polar SDK (Standard Webhooks). Subscription state
  forwarding to `crates/server` is a TODO until M7 lands.
- `src/lib/plans.ts`: plan definitions shared by the pricing page and the
  billing layer.

Polar has a full sandbox (`POLAR_SERVER=sandbox`) to test the whole
checkout + webhook flow before the production store is approved.

Configuration is environment-only; copy `.env.example` and fill in the
store, variant IDs and webhook secret. Without configuration the pricing
page shows "Checkout is not available yet on this deployment" instead of a
broken flow.
