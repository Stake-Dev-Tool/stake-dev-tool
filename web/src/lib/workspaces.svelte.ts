/**
 * web/src/lib/workspaces.svelte.ts
 *
 * Session-cached workspace list, shared by the Nav's WorkspaceSwitcher and the
 * breadcrumbs (which resolve a slug → display name). One `GET /workspaces` per
 * session backs every switcher and every breadcrumb, instead of a fetch per page.
 *
 * The `/` (workspaces) page owns the authoritative list + its own error state;
 * it seeds this cache on load so the switcher is populated without a second call.
 */
import { api, type WorkspaceMembership } from './api';

export const workspacesStore = $state<{ items: WorkspaceMembership[]; loaded: boolean }>({
  items: [],
  loaded: false
});

let inflight: Promise<void> | null = null;

/**
 * Ensure the workspace list is loaded. Concurrent callers share one request; a
 * resolved list is reused for the session unless `force` re-fetches. Never throws
 * — a failed load simply leaves the cache empty (the switcher stays quiet).
 */
export function loadWorkspaces(force = false): Promise<void> {
  if (!force && workspacesStore.loaded) return Promise.resolve();
  if (inflight) return inflight;
  inflight = api.workspaces
    .list()
    .then((list) => {
      workspacesStore.items = list;
      workspacesStore.loaded = true;
    })
    .catch(() => {
      // Leave whatever we had; the switcher just won't show fresh entries.
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

/** Seed the cache from an already-fetched list (the `/` page does this). */
export function setWorkspaces(items: WorkspaceMembership[]): void {
  workspacesStore.items = items;
  workspacesStore.loaded = true;
}

/** Clear the cache (on logout, so the next user never sees stale entries). */
export function resetWorkspaces(): void {
  workspacesStore.items = [];
  workspacesStore.loaded = false;
}

/** Resolve a slug to its workspace display name, falling back to the slug. */
export function workspaceName(slug: string): string {
  if (!slug) return '';
  return workspacesStore.items.find((m) => m.workspace.slug === slug)?.workspace.name ?? slug;
}
