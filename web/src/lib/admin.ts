/**
 * web/src/lib/admin.ts — a session-cached "is this user an instance admin?" probe.
 *
 * Mirrors billing.ts's module-cache idiom: one `GET /api/admin/me` per session,
 * shared by every mount that needs it (the Nav's Admin link today). Non-admins
 * get a flat 404, which `api.admin.me()` resolves to `false` — a stable answer
 * we keep cached. A transient (non-404) failure rejects and is evicted so a
 * later mount re-probes rather than caching a miss forever. Callers treat any
 * rejection as "not admin" and simply hide the surface.
 */
import { api } from './api';

let probe: Promise<boolean> | null = null;

/**
 * Resolve (and cache for the session) whether the current session is an instance
 * admin. Concurrent callers share one in-flight request; a resolved answer is
 * reused. A rejected probe is evicted so the next call re-tries.
 */
export function isAdmin(): Promise<boolean> {
  if (!probe) {
    const p = api.admin.me();
    probe = p;
    p.catch(() => {
      // Only non-404 failures reach here (404 resolves to `false`). Evict so a
      // later mount can re-probe instead of being wedged by a transient error.
      if (probe === p) probe = null;
    });
  }
  return probe;
}

/** Clear the cached probe (on logout, so the next user re-probes fresh). */
export function resetAdmin(): void {
  probe = null;
}
