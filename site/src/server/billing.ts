import { createServerFn } from '@tanstack/react-start'
import type { BillingInterval, PlanId } from '../lib/plans'

/**
 * Billing runs through a merchant of record (Lemon Squeezy) so VAT on
 * cross-border B2C sales is remitted for us — see V2.md. This module is the
 * only place that talks to their API; the rest of the site only ever sees a
 * checkout URL.
 *
 * All configuration comes from the environment (see site/.env.example).
 * Without it, checkout reports `not_configured` and the pricing page says so
 * instead of half-working.
 */

type BillingConfig = {
  apiKey: string
  storeId: string
  siteUrl: string
  variants: Record<PlanId, Record<BillingInterval, string | undefined>>
}

function getBillingConfig(): BillingConfig | null {
  const apiKey = process.env.LEMONSQUEEZY_API_KEY
  const storeId = process.env.LEMONSQUEEZY_STORE_ID
  if (!apiKey || !storeId) return null

  return {
    apiKey,
    storeId,
    siteUrl: process.env.SITE_URL ?? 'http://localhost:3000',
    variants: {
      solo: {
        month: process.env.LEMONSQUEEZY_VARIANT_SOLO_MONTHLY,
        year: process.env.LEMONSQUEEZY_VARIANT_SOLO_YEARLY,
      },
      team: {
        month: process.env.LEMONSQUEEZY_VARIANT_TEAM_MONTHLY,
        year: process.env.LEMONSQUEEZY_VARIANT_TEAM_YEARLY,
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
    const variantId = config?.variants[data.plan][data.interval]
    if (!config || !variantId) {
      return { url: null, reason: 'not_configured' }
    }

    const response = await fetch('https://api.lemonsqueezy.com/v1/checkouts', {
      method: 'POST',
      headers: {
        Accept: 'application/vnd.api+json',
        'Content-Type': 'application/vnd.api+json',
        Authorization: `Bearer ${config.apiKey}`,
      },
      body: JSON.stringify({
        data: {
          type: 'checkouts',
          attributes: {
            checkout_data: {
              custom: { plan: data.plan, interval: data.interval },
            },
            product_options: {
              redirect_url: `${config.siteUrl}/checkout/success`,
            },
          },
          relationships: {
            store: { data: { type: 'stores', id: config.storeId } },
            variant: { data: { type: 'variants', id: variantId } },
          },
        },
      }),
    })

    if (!response.ok) {
      console.error(
        `[billing] checkout creation failed: ${response.status} ${await response.text()}`,
      )
      return { url: null, reason: 'provider_error' }
    }

    const payload = (await response.json()) as {
      data?: { attributes?: { url?: string } }
    }
    const url = payload.data?.attributes?.url
    if (!url) {
      console.error('[billing] checkout response had no URL')
      return { url: null, reason: 'provider_error' }
    }

    return { url, reason: null }
  })
