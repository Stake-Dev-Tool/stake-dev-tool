/**
 * web/src/lib/session.svelte.ts
 *
 * Client-side auth state. A single `$state` object, mutated in place, imported
 * anywhere as `session`. `loaded` flips true after the first /auth/me round-trip
 * so the layout guard doesn't bounce users to /login before we know who they are.
 */
import { api, type User } from './api';

export const session = $state<{ user: User | null; loaded: boolean }>({
  user: null,
  loaded: false
});

/** Fetch the current user once (or again after login/logout). Never throws. */
export async function refreshSession(): Promise<void> {
  try {
    session.user = await api.auth.me();
  } catch {
    session.user = null;
  } finally {
    session.loaded = true;
  }
}

export function setUser(user: User | null): void {
  session.user = user;
  session.loaded = true;
}
