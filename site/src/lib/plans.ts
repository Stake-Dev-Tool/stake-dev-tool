export type PlanId = 'solo' | 'team'
export type BillingInterval = 'month' | 'year'

export const PLANS: Record<
  PlanId,
  {
    name: string
    monthly: number
    yearly: number
    features: Array<string>
  }
> = {
  solo: {
    name: 'Solo',
    monthly: 5,
    yearly: 48,
    features: [
      '1 user, unlimited games',
      '10 GB math storage',
      'Share links, fair-use sessions',
      '14-day free trial',
    ],
  },
  team: {
    name: 'Team',
    monthly: 15,
    yearly: 144,
    features: [
      'Up to 10 members',
      '50 GB math storage',
      'Higher share-session quotas',
      'Custom play subdomain',
    ],
  },
}

export const SELF_HOST_FEATURES = [
  'Every feature, no exceptions',
  'Single binary + Postgres + Caddy',
  'Your infra, your data',
  'Community support',
]
