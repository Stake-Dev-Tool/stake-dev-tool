# site

Marketing site for Stake Dev Tool (stakedevtool.com), built with
[TanStack Start](https://tanstack.com/start), React and Tailwind CSS v4.

## Develop

```bash
pnpm site:dev      # from the repo root, serves on http://localhost:3000
```

## Deploy

The site deploys to Vercel automatically: every push to `v2` that touches
`site/` (or the root lockfile) triggers a production build via the Vercel
Git integration. Nothing to run by hand.

- Vercel project: `stake-dev-tool-site` (Root Directory: `site`,
  production branch: `v2`).
- `site/vercel.json` holds the install command and the ignore rule that
  skips builds for commits that don't touch the site.
- The repo-root `vercel.json` is a guard: it skips any build that would
  run with a misconfigured Root Directory.

CI (`.github/workflows/site-ci.yml`) type-checks and builds the site on
every push/PR touching `site/` — Vercel only ships what CI also builds.

## Structure

- `src/routes/`: one file per page — `index` (home), `features`, `cloud`,
  `pricing`, `cli` (CLI + MCP install), `open-source`, plus the legal set
  (`legal`, `terms`, `privacy`, `refunds`).
- `src/components/TestViewFigure.tsx`: the hero figure. The same mini slot
  front at three resolutions inside one app window, with a live SSE event
  ticker.
- `src/styles.css`: design tokens (spruce/mint/amber palette, Bricolage
  Grotesque + Geist type) and the few custom CSS pieces (ticker, frames,
  section rules).

## Billing

The site itself takes no payments. All checkout happens in the dashboard
(app.stakedevtool.com) through Stripe as merchant of record; the pricing
page links there. Keep pricing copy in sync with `crates/server` plan
limits.
