/**
 * web/src/lib/billing.ts — billing presentation helpers + a tiny per-slug status
 * cache (no component state; pure functions + a module Map).
 *
 * PlanBanner mounts on the workspace page and both game pages; the cache means
 * those (up to) three mounts share a single `GET /billing` per slug per session.
 * The billing page seeds the cache with a fresh read after a checkout returns so
 * every surface reflects the new subscription without another round-trip.
 */
import { api, type BillingInterval, type BillingStatus } from './api';

// ---------------------------------------------------------------------------
// Per-slug status cache
// ---------------------------------------------------------------------------

/** Resolved-or-in-flight status per workspace slug, shared across mounts. */
const cache = new Map<string, Promise<BillingStatus>>();

/**
 * Fetch (or reuse) the workspace's billing status. Concurrent callers share one
 * request; a resolved entry is reused for the whole session. A rejected read is
 * evicted so a transient failure never wedges the slug.
 */
export function billingStatus(slug: string): Promise<BillingStatus> {
  let hit = cache.get(slug);
  if (!hit) {
    hit = api.billing.status(slug);
    cache.set(slug, hit);
    hit.catch(() => {
      if (cache.get(slug) === hit) cache.delete(slug);
    });
  }
  return hit;
}

/** Drop the cached status for a slug (e.g. right after an upgrade). */
export function invalidateBillingStatus(slug: string): void {
  cache.delete(slug);
}

/** Seed the cache with an already-fetched status so mounts skip the network. */
export function setBillingStatus(slug: string, status: BillingStatus): void {
  cache.set(slug, Promise.resolve(status));
}

// ---------------------------------------------------------------------------
// Presentation helpers (pure)
// ---------------------------------------------------------------------------

/** Whole days until an ISO instant, rounded up and clamped ≥ 0. */
export function daysUntil(iso: string | null | undefined): number {
  if (!iso) return 0;
  const ms = new Date(iso).getTime() - Date.now();
  if (!Number.isFinite(ms)) return 0;
  return Math.max(0, Math.ceil(ms / 86_400_000));
}

/** Human plan name for a resolved plan label. */
export function planLabel(plan: string): string {
  switch (plan) {
    case 'trial':
      return 'Trial';
    case 'solo':
      return 'Solo';
    case 'team':
      return 'Team';
    case 'unlimited':
      return 'Unlimited';
    case 'expired':
      return 'Trial expired';
    default:
      return plan ? plan[0].toUpperCase() + plan.slice(1) : '—';
  }
}

/** Humanize Polar's verbatim subscription status ("past_due" → "Past due"). */
export function statusLabel(status: string | null | undefined): string {
  if (!status) return '—';
  return status
    .split('_')
    .map((w) => (w ? w[0].toUpperCase() + w.slice(1) : w))
    .join(' ');
}

/** "Monthly" / "Yearly" / "—". */
export function intervalLabel(interval: BillingInterval | null | undefined): string {
  if (interval === 'monthly') return 'Monthly';
  if (interval === 'yearly') return 'Yearly';
  return '—';
}

export type MeterTone = 'accent' | 'warn' | 'danger';

/**
 * A usage meter against a limit. `null` limit = unlimited (no fill, no warning).
 * Fill turns amber at ≥ 80% and red at 100%.
 */
export function meter(
  usage: number,
  limit: number | null
): { pct: number; tone: MeterTone; unlimited: boolean } {
  if (limit == null) return { pct: 0, tone: 'accent', unlimited: true };
  if (limit <= 0) return { pct: 100, tone: 'danger', unlimited: false };
  const pct = Math.min(100, Math.max(0, Math.round((usage / limit) * 100)));
  const tone: MeterTone = pct >= 100 ? 'danger' : pct >= 80 ? 'warn' : 'accent';
  return { pct, tone, unlimited: false };
}

/** Tailwind background class for a meter tone. */
export function meterFill(tone: MeterTone): string {
  return tone === 'danger' ? 'bg-danger' : tone === 'warn' ? 'bg-warn' : 'bg-accent';
}
