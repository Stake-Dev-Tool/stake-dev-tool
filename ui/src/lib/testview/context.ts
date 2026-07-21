// TestView injection context.
//
// The test view was previously hardwired to the desktop shell: it read its
// config from `page.url.searchParams`, talked to the LGS at `location.origin`,
// and minted session ids from `stake-dev-tool:<slug>:<resId>`. `TestViewContext`
// lifts every one of those ambient inputs into an injected object so the same
// `TestView.svelte` body can run against the desktop LGS today and, on the cloud
// server (M6), the SAME embedded test view served under a tenant prefix.
//
// KEY M6 INSIGHT: the built `/test/` page is embedded in the lgs crate and served
// by every tenant router, so it is reachable both at `/test/` (desktop) and at
// `/api/ws/<ws>/g/<game>/r/<n>/test/` (cloud), same-origin with the dashboard
// (session cookie present). The ONLY thing that differs is the base prefix. So
// `resolveContext` sniffs the tenant prefix off `location.pathname`: found ⇒
// cloud context (every devtool/RGS/replay URL re-based under the prefix); absent
// ⇒ the byte-identical desktop context (the zero-regression bar).
//
// This module is deliberately Tauri-free (it must stay reachable from `/test`).

export type TestViewCapabilities = {
  /** Manage the resolution matrix (toggle / add / delete presets). */
  manageResolutions: boolean;
  /** Force the next event for a mode. */
  forceEvent: boolean;
  /** Save / bookmark rounds. */
  saveRounds: boolean;
  /** Reset all sessions. */
  reset: boolean;
  /** Open a frame in a new browser tab. */
  openInBrowser: boolean;
};

export type TestViewContext = {
  /**
   * Prefix for every devtool HTTP/SSE/prepare call. `''` (desktop) keeps the
   * calls same-origin relative; a cloud/share workbench passes a
   * workspace/game/revision-scoped prefix or share origin.
   */
  apiBase: string;
  /** Base URL of the game front loaded into the iframe (was the `gameUrl` query param). */
  frontUrl: string;
  /** Game slug the LGS resolves math against. */
  gameSlug: string;
  /** Mint the per-resolution session id (`stake-dev-tool:<slug>:<resId>` on desktop). */
  makeSessionId: (resId: string) => string;
  /**
   * Full `rgs_url` for the game front's play contract (the iframe query param):
   * `<host><prefix>/api/rgs/<slug>`. The front + RGS are always same-origin, so
   * the base is the browser authority (`location.host`) — NOT `apiBase`, which is
   * an origin-relative path used by `fetch`. Desktop returns the pre-M6 value
   * `<host>/api/rgs/<slug>` byte-identically; cloud splices the tenant prefix in.
   */
  rgsUrl: (gameSlug: string) => string;
  /**
   * Authority(+prefix) base handed to the replay front as its `rgs_url` (the
   * replay contract passes the bare base and a separate `game` param, so this
   * carries no `/api/rgs`). Desktop → `<host>` (byte-identical); cloud →
   * `<host><prefix>`.
   */
  rgsBase: () => string;
  /**
   * Prefix inserted into the test view's `localStorage` keys. `''` on desktop
   * (keys unchanged — byte-identical); on the cloud it carries the tenant prefix
   * so two tenants sharing one browser origin never read each other's persisted
   * per-frame session ids / viewport choices (recon B.4, M6 task #6). Note this
   * namespaces only the storage KEYS: the session id VALUE from `makeSessionId`
   * is intentionally unchanged (M4 defers client-id namespacing to the server,
   * which prefixes `<user_id>:` and keys a distinct store per tenant router).
   */
  storageNamespace: string;
  /** Feature gates — all enabled on desktop; a cloud/share host narrows them. */
  capabilities: TestViewCapabilities;
  /** Optional scoping for a cloud workbench (unused on desktop). */
  auth?: { workspace?: string; revision?: string };
};

// Both the desktop wrapper and the cloud mount hand us the window's location for
// its `.host` (RGS base) and `.pathname` (prefix detection). A `URL` (SvelteKit's
// `page.url`) and a `Location` both satisfy this shape.
type LocationLike = Pick<Location, 'host' | 'pathname'> | URL;

function hostOf(location?: LocationLike): string {
  // Browser-only module (the app runs with `ssr = false`), so the global
  // `location` is a safe fallback when a caller omits the argument.
  return (location ?? globalThis.location).host;
}

/**
 * Match the tenant prefix the cloud server mounts the LGS (and thus this test
 * view) under: `…/api/ws/<slug>/g/<game>/r/<number>/`. Returns the prefix up to
 * and including the revision number (no trailing slash), or `null` when the page
 * is not served under a tenant mount (i.e. the desktop `/test/`).
 *
 * Example: `/api/ws/acme/g/demo/r/3/test/` → `/api/ws/acme/g/demo/r/3`.
 */
export function detectTenantPrefix(pathname: string): string | null {
  const m = pathname.match(/^(.*\/api\/ws\/[^/]+\/g\/[^/]+\/r\/\d+)\//);
  return m ? m[1] : null;
}

/**
 * Desktop context factory. Reproduces the pre-refactor test view behavior
 * byte-identically:
 *  - `apiBase = ''`   → devtool HTTP/SSE/prepare stay same-origin relative paths.
 *  - `rgsUrl`/`rgsBase` → `<host>/api/rgs/<slug>` / `<host>` (was `location.host`).
 *  - `frontUrl`       ← the `gameUrl` query param.
 *  - `gameSlug`       ← the `gameSlug` query param.
 *  - session id format `stake-dev-tool:<slug>:<resId>`.
 *  - `storageNamespace = ''` → localStorage keys unchanged.
 *  - all capabilities enabled (single-user desktop).
 */
export function desktopContext(
  searchParams: URLSearchParams,
  location?: LocationLike
): TestViewContext {
  const host = hostOf(location);
  const gameSlug = searchParams.get('gameSlug') ?? '';
  const frontUrl = searchParams.get('gameUrl') ?? '';
  return {
    apiBase: '',
    frontUrl,
    gameSlug,
    makeSessionId: (resId: string) => `stake-dev-tool:${gameSlug}:${resId}`,
    rgsUrl: (slug: string) => `${host}/api/rgs/${slug}`,
    rgsBase: () => host,
    storageNamespace: '',
    capabilities: {
      manageResolutions: true,
      forceEvent: true,
      saveRounds: true,
      reset: true,
      openInBrowser: true
    }
  };
}

/**
 * Cloud workbench context factory. Same embedded test view, re-based under the
 * tenant `prefix` (`/api/ws/<slug>/g/<game>/r/<number>`) the cloud server mounts
 * the LGS at:
 *  - `apiBase = prefix` → devtool HTTP/SSE/prepare hit `<origin><prefix>/api/devtool/…`
 *    (origin-relative, so the browser resolves them against the current origin —
 *    the session cookie rides along same-origin).
 *  - `rgsUrl`/`rgsBase` splice the prefix in → the game front + replay hit
 *    `<host><prefix>/api/rgs/…`.
 *  - `frontUrl`/`gameSlug` from the same query params as desktop (the caller — the
 *    web workbench "Open test view" affordance — supplies them).
 *  - `makeSessionId` UNCHANGED (see `storageNamespace` note on the type).
 *  - `storageNamespace = "<prefix>:"` → per-tenant localStorage isolation.
 *  - all capabilities enabled; every one talks to the tenant-scoped inner LGS
 *    (settings/reset are per-tenant on the cloud, so they are safe for M6).
 */
export function cloudContext(
  searchParams: URLSearchParams,
  location: LocationLike,
  prefix: string
): TestViewContext {
  const host = hostOf(location);
  const gameSlug = searchParams.get('gameSlug') ?? '';
  const frontUrl = searchParams.get('gameUrl') ?? '';
  // Pull workspace slug + revision number out of the detected prefix for the
  // (informational) auth scope. Game slug already travels as `gameSlug`.
  const seg = prefix.match(/\/api\/ws\/([^/]+)\/g\/[^/]+\/r\/(\d+)$/);
  return {
    apiBase: prefix,
    frontUrl,
    gameSlug,
    // UNCHANGED from desktop on purpose (M6 task #6 / recon B.4): the client id
    // value stays as-is; only the storage keys below carry the tenant prefix.
    makeSessionId: (resId: string) => `stake-dev-tool:${gameSlug}:${resId}`,
    rgsUrl: (slug: string) => `${host}${prefix}/api/rgs/${slug}`,
    rgsBase: () => `${host}${prefix}`,
    storageNamespace: `${prefix}:`,
    capabilities: {
      manageResolutions: true,
      forceEvent: true,
      saveRounds: true,
      reset: true,
      openInBrowser: true
    },
    auth: seg ? { workspace: seg[1], revision: seg[2] } : undefined
  };
}

/**
 * Resolve the right context from the window's URL. Used by the `/test` wrapper:
 * a tenant prefix on the path ⇒ cloud context; otherwise the byte-identical
 * desktop context. `searchParams` is passed separately because a `Location` (as
 * opposed to a `URL`) exposes no `searchParams`.
 */
export function resolveContext(
  searchParams: URLSearchParams,
  location: LocationLike
): TestViewContext {
  const prefix = detectTenantPrefix(location.pathname);
  return prefix
    ? cloudContext(searchParams, location, prefix)
    : desktopContext(searchParams, location);
}
