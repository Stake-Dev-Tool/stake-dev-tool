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

- `src/routes/index.tsx`: the landing page (hero, features, cloud/V2,
  pricing, open source, FAQ).
- `src/components/TestViewFigure.tsx`: the hero figure. Nested viewport
  frames with a live SSE event ticker, echoing the multi-resolution test
  view.
- `src/styles.css`: design tokens (spruce/mint/amber palette, Bricolage
  Grotesque + Geist type) and the few custom CSS pieces (ticker, frames,
  section rules).

Content mirrors `V2.md`. Keep pricing and cloud copy in sync with it.
