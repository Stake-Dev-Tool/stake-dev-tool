import { Polar } from '@polar-sh/sdk'
import { createServerFn } from '@tanstack/react-start'
import type { BillingInterval, PlanId } from '../lib/plans'

/**
 * Billing runs through Polar (polar.sh) as merchant of record, so VAT on
 * cross-border B2C sales is remitted for us — see V2.md. This module is the
 * only place that talks to their API; the rest of the site only ever sees a
 * checkout URL.
 *
 * All configuration comes from the environment (see site/.env.example).
 * Without it, checkout reports `not_configured` and the pricing page says so
 * instead of half-working. Set POLAR_SERVER=sandbox to test the full flow
 * against Polar's sandbox environment.
 */

type BillingConfig = {
  accessToken: string
  server: 'production' | 'sandbox'
  siteUrl: string
  products: Record<PlanId, Record<BillingInterval, string | undefined>>
}

function getBillingConfig(): BillingConfig | null {
  const accessToken = process.env.POLAR_ACCESS_TOKEN
  if (!accessToken) return null

  return {
    accessToken,
    server: process.env.POLAR_SERVER === 'sandbox' ? 'sandbox' : 'production',
    siteUrl: process.env.SITE_URL ?? 'http://localhost:3000',
    products: {
      solo: {
        month: process.env.POLAR_PRODUCT_SOLO_MONTHLY,
        year: process.env.POLAR_PRODUCT_SOLO_YEARLY,
      },
      team: {
        month: process.env.POLAR_PRODUCT_TEAM_MONTHLY,
        year: process.env.POLAR_PRODUCT_TEAM_YEARLY,
      },
    },
  }
}

export type CheckoutResult =
  | { url: string; reason: null }
  | { url: null; reason: 'not_configured' | 'provider_error' }

export const createCheckout = createServerFn({ method: 'POST' })
  .validator((data: { plan: PlanId; interval: BillingInterval }) => {
    if (data.plan !== 'solo' && data.plan !== 'team') {
      throw new Error(`Unknown plan: ${String(data.plan)}`)
    }
    if (data.interval !== 'month' && data.interval !== 'year') {
      throw new Error(`Unknown interval: ${String(data.interval)}`)
    }
    return data
  })
  .handler(async ({ data }): Promise<CheckoutResult> => {
    const config = getBillingConfig()
    const productId = config?.products[data.plan][data.interval]
    if (!config || !productId) {
      return { url: null, reason: 'not_configured' }
    }

    try {
      const polar = new Polar({
        accessToken: config.accessToken,
        server: config.server,
      })
      const checkout = await polar.checkouts.create({
        products: [productId],
        successUrl: `${config.siteUrl}/checkout/success?checkout_id={CHECKOUT_ID}`,
        metadata: { plan: data.plan, interval: data.interval },
      })
      return { url: checkout.url, reason: null }
    } catch (error) {
      console.error('[billing] checkout creation failed:', error)
      return { url: null, reason: 'provider_error' }
    }
  })
