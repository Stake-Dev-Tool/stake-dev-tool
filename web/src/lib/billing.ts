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
    case 'free':
      return 'Free';
    case 'paid':
      return 'Paid';
    case 'unlimited':
      return 'Unlimited';
    default:
      return plan ? plan[0].toUpperCase() + plan.slice(1) : '—';
  }
}

/** Humanize Stripe's verbatim subscription status ("past_due" → "Past due"). */
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

// ---------------------------------------------------------------------------
// Seat pricing (the single paid plan: €3 first seat, €2 each additional)
// ---------------------------------------------------------------------------

/** Monthly price (EUR) of the first seat. */
export const SEAT_FIRST_EUR = 3;
/** Monthly price (EUR) of each additional seat. */
export const SEAT_ADDITIONAL_EUR = 2;
/** Yearly billing charges 10 months (2 months free). */
export const YEARLY_MONTHS = 10;
/** Inclusive bounds on the seat count (mirrors the server's 1..=100). */
export const SEATS_MIN = 1;
export const SEATS_MAX = 100;

/** Clamp a seat count into the purchasable range. */
export function clampSeats(seats: number): number {
  if (!Number.isFinite(seats)) return SEATS_MIN;
  return Math.min(SEATS_MAX, Math.max(SEATS_MIN, Math.round(seats)));
}

/** Monthly price (EUR) for `seats`: €3 first seat + €2 each additional. */
export function seatMonthlyEur(seats: number): number {
  const n = clampSeats(seats);
  return SEAT_FIRST_EUR + SEAT_ADDITIONAL_EUR * (n - 1);
}

/** Yearly price (EUR) for `seats`: the monthly total × 10 (2 months free). */
export function seatYearlyEur(seats: number): number {
  return seatMonthlyEur(seats) * YEARLY_MONTHS;
}

// ---------------------------------------------------------------------------
// Storage add-on (one unit = +10 GiB for €1/mo, quantity-based)
// ---------------------------------------------------------------------------

/** GiB granted per storage add-on unit. */
export const STORAGE_UNIT_GIB = 10;
/** Monthly price (EUR) of one storage add-on unit. */
export const STORAGE_UNIT_PRICE_EUR = 1;
/** Inclusive bounds on a single storage purchase (mirrors the server's 1..=100). */
export const STORAGE_UNITS_MIN = 1;
export const STORAGE_UNITS_MAX = 100;

/** Clamp a unit count into the purchasable range. */
export function clampStorageUnits(units: number): number {
  if (!Number.isFinite(units)) return STORAGE_UNITS_MIN;
  return Math.min(STORAGE_UNITS_MAX, Math.max(STORAGE_UNITS_MIN, Math.round(units)));
}

/** Monthly price (EUR) for `units` storage add-on units. */
export function storageMonthlyEur(units: number): number {
  return Math.max(0, units) * STORAGE_UNIT_PRICE_EUR;
}

// ---------------------------------------------------------------------------
// Entitlements ("what you get") — the caps one seat grants, and the totals for a
// chosen seat count. Mirrors the server's per-seat scaling in billing/plan.rs:
// each seat = 1 member + 10 GiB storage + 5 active share links + 5 live sessions.
// ---------------------------------------------------------------------------

/** What a single seat grants — the unit shown next to the stepper. */
export const PER_SEAT = {
  members: 1,
  storageGib: STORAGE_UNIT_GIB, // 10
  shareLinks: 5,
  sessions: 5
} as const;

export interface SeatEntitlements {
  members: number;
  storageGib: number;
  shareLinks: number;
  sessions: number;
}

/** Total entitlements granted by `seats` seats (before any storage add-on). */
export function seatEntitlements(seats: number): SeatEntitlements {
  const n = clampSeats(seats);
  return {
    members: n * PER_SEAT.members,
    storageGib: n * PER_SEAT.storageGib,
    shareLinks: n * PER_SEAT.shareLinks,
    sessions: n * PER_SEAT.sessions
  };
}

// ---------------------------------------------------------------------------
// Combined price summary (seats + optional storage add-on), updated live as the
// steppers move. The storage add-on is always billed monthly (€1/unit/mo); only
// the seat portion has a yearly cadence (2 months free).
// ---------------------------------------------------------------------------

export interface PriceSummary {
  /** Seat subtotal per month (€3 first seat + €2 each additional). */
  seatMonthly: number;
  /** Seat subtotal per year (monthly × 10 — 2 months free). */
  seatYearly: number;
  /** Storage add-on subtotal per month (€1 × units). */
  storageMonthly: number;
  /** Grand total per month = seatMonthly + storageMonthly. */
  monthlyTotal: number;
  /** What the yearly cadence saves on the seat portion (2 months). */
  yearlySaving: number;
}

/** Live combined pricing for `seats` seats plus `storageUnits` storage units. */
export function priceSummary(seats: number, storageUnits: number): PriceSummary {
  const seatMonthly = seatMonthlyEur(seats);
  const seatYearly = seatYearlyEur(seats);
  const storageMonthly = storageMonthlyEur(Math.max(0, storageUnits));
  return {
    seatMonthly,
    seatYearly,
    storageMonthly,
    monthlyTotal: seatMonthly + storageMonthly,
    yearlySaving: seatMonthly * (12 - YEARLY_MONTHS)
  };
}
