export type BillingInterval = 'month' | 'year'

// The single paid plan is billed per seat: €3/mo for the first seat, €2/mo for
// each additional seat. Yearly = 10 months (2 months free).
export const SEAT_FIRST_EUR = 3
export const SEAT_ADDITIONAL_EUR = 2
export const YEARLY_MONTHS = 10
export const SEATS_MIN = 1
export const SEATS_MAX = 100

/** Clamp a seat count into the purchasable range. */
export function clampSeats(seats: number): number {
  if (!Number.isFinite(seats)) return SEATS_MIN
  return Math.min(SEATS_MAX, Math.max(SEATS_MIN, Math.round(seats)))
}

/** Monthly price (EUR) for `seats`: €3 first seat + €2 each additional. */
export function seatMonthlyEur(seats: number): number {
  const n = clampSeats(seats)
  return SEAT_FIRST_EUR + SEAT_ADDITIONAL_EUR * (n - 1)
}

/** Yearly price (EUR) for `seats`: the monthly total × 10 (2 months free). */
export function seatYearlyEur(seats: number): number {
  return seatMonthlyEur(seats) * YEARLY_MONTHS
}

// Per-seat quotas, for the features list on the pricing page.
export const PLAN_FEATURES = [
  'One member slot per seat',
  '10 GiB math storage per seat',
  '5 active share links per seat',
  '5 live play sessions per seat',
  'Unlimited games and revisions',
  'Custom play subdomain · cancel anytime',
]

export const SELF_HOST_FEATURES = [
  'Every feature, no exceptions',
  'Single binary + Postgres + Caddy',
  'Your infra, your data',
  'Community support',
]
