# Dashboard browser regression tests

`build-clarity.cjs` exercises the real Svelte UI with explicit local API fixtures.
It checks build discoverability and navigation; it does **not** prove backend
authorization or production access. Backend download/security regressions live
in `crates/server/tests/math_revisions.rs` (`build_archive` tests, with a real
`TEST_DATABASE_URL`).

From the repository root, use a separate terminal for the dev server:

```sh
corepack pnpm@10 install --frozen-lockfile --filter web
corepack pnpm@10 --filter web exec vite --host 127.0.0.1 --port 5193 --strictPort
```

The tests require Playwright and its Chromium browser. An isolated installation
keeps the dashboard dependency lockfile unchanged (verified with Playwright 1.63.0):

```sh
npm install --prefix /tmp/sdt-playwright --no-save playwright@1.63.0
/tmp/sdt-playwright/node_modules/.bin/playwright install chromium
PLAYWRIGHT_MODULE=/tmp/sdt-playwright/node_modules/playwright node web/tests/build-clarity.cjs
```

`SDT_TEST_URL` overrides the default `http://127.0.0.1:5193` when testing a different
local server. API responses are intercepted; never interpret this as a production
end-to-end test.

`share-versions.cjs` covers independent math/front share pins, explicit latest
payloads, role gates, and loading/error/navigation safety. Its default URL uses
port 5194; it can reuse the server above with:

```sh
SDT_TEST_URL=http://127.0.0.1:5193 PLAYWRIGHT_MODULE=/tmp/sdt-playwright/node_modules/playwright node web/tests/share-versions.cjs
```

Stop Vite before running the final `corepack pnpm@10 --filter web check` and
`corepack pnpm@10 --filter web build`, and serialize those commands. Concurrent
SvelteKit sync/build processes can invalidate each other's generated output.
