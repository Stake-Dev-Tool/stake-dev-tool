import { createHmac, timingSafeEqual } from 'node:crypto'
import { createFileRoute } from '@tanstack/react-router'

/**
 * Lemon Squeezy webhook receiver. Signature is an HMAC-SHA256 of the raw
 * body with the shared webhook secret, sent in the X-Signature header.
 *
 * Subscription state belongs to crates/server (M7); this endpoint verifies
 * and acknowledges events so the store can be wired up before that lands,
 * then forwards once the server exposes its billing API.
 */

function verifySignature(rawBody: string, signature: string, secret: string): boolean {
  const digest = createHmac('sha256', secret).update(rawBody).digest('hex')
  const a = Buffer.from(digest, 'utf8')
  const b = Buffer.from(signature, 'utf8')
  return a.length === b.length && timingSafeEqual(a, b)
}

type LemonSqueezyEvent = {
  meta?: {
    event_name?: string
    custom_data?: { plan?: string; interval?: string }
  }
  data?: { id?: string; type?: string }
}

export const Route = createFileRoute('/api/billing/webhook')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const secret = process.env.LEMONSQUEEZY_WEBHOOK_SECRET
        if (!secret) {
          return new Response('webhook not configured', { status: 503 })
        }

        const rawBody = await request.text()
        const signature = request.headers.get('x-signature') ?? ''
        if (!verifySignature(rawBody, signature, secret)) {
          return new Response('invalid signature', { status: 401 })
        }

        let event: LemonSqueezyEvent
        try {
          event = JSON.parse(rawBody) as LemonSqueezyEvent
        } catch {
          return new Response('invalid payload', { status: 400 })
        }

        const eventName = event.meta?.event_name ?? 'unknown'
        switch (eventName) {
          case 'order_created':
          case 'subscription_created':
          case 'subscription_updated':
          case 'subscription_cancelled':
          case 'subscription_expired':
          case 'subscription_payment_failed':
            // TODO(M7): forward to crates/server so workspace quotas and
            // access follow the subscription state.
            console.log(
              `[billing] ${eventName} for ${event.data?.type ?? '?'}#${event.data?.id ?? '?'}`,
              event.meta?.custom_data ?? {},
            )
            break
          default:
            console.log(`[billing] ignoring event: ${eventName}`)
        }

        return Response.json({ received: true })
      },
    },
  },
})
