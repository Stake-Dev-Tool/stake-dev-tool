// TestView injection context.
//
// The test view was previously hardwired to the desktop shell: it read its
// config from `page.url.searchParams`, talked to the LGS at `location.origin`,
// and minted session ids from `stake-dev-tool:<slug>:<resId>`. `TestViewContext`
// lifts every one of those ambient inputs into an injected object so the same
// `TestView.svelte` body can run against the desktop LGS today and, later, a
// cloud/share workbench (`cloudContext`/`shareContext`, not built yet).
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
  /** Feature gates — all enabled on desktop; a cloud/share host narrows them. */
  capabilities: TestViewCapabilities;
  /** Optional scoping for a cloud workbench (unused on desktop). */
  auth?: { workspace?: string; revision?: string };
};

/**
 * Desktop context factory. Reproduces the pre-refactor test view behavior
 * byte-identically:
 *  - `apiBase = ''`   → devtool HTTP/SSE/prepare stay same-origin relative paths.
 *  - `frontUrl`       ← the `gameUrl` query param.
 *  - `gameSlug`       ← the `gameSlug` query param.
 *  - session id format `stake-dev-tool:<slug>:<resId>`.
 *  - all capabilities enabled (single-user desktop).
 *
 * `location` is accepted for signature parity with the (future) cloud/share
 * factories; the desktop base is same-origin, so it is not needed here.
 */
export function desktopContext(
  searchParams: URLSearchParams,
  _location?: Location
): TestViewContext {
  const gameSlug = searchParams.get('gameSlug') ?? '';
  const frontUrl = searchParams.get('gameUrl') ?? '';
  return {
    apiBase: '',
    frontUrl,
    gameSlug,
    makeSessionId: (resId: string) => `stake-dev-tool:${gameSlug}:${resId}`,
    capabilities: {
      manageResolutions: true,
      forceEvent: true,
      saveRounds: true,
      reset: true,
      openInBrowser: true
    }
  };
}
