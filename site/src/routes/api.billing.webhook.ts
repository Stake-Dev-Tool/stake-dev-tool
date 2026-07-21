import { createFileRoute } from '@tanstack/react-router'
import { WebhookVerificationError, validateEvent } from '@polar-sh/sdk/webhooks'

/**
 * Polar webhook receiver. Polar signs deliveries per the Standard Webhooks
 * spec (webhook-id / webhook-timestamp / webhook-signature headers); the SDK
 * helper verifies the signature and returns the typed event.
 *
 * Subscription state belongs to crates/server (M7); this endpoint verifies
 * and acknowledges events so the store can be wired up before that lands,
 * then forwards once the server exposes its billing API.
 */

export const Route = createFileRoute('/api/billing/webhook')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const secret = process.env.POLAR_WEBHOOK_SECRET
        if (!secret) {
          return new Response('webhook not configured', { status: 503 })
        }

        const rawBody = await request.text()
        const headers = Object.fromEntries(request.headers.entries())

        let event: ReturnType<typeof validateEvent>
        try {
          event = validateEvent(rawBody, headers, secret)
        } catch (error) {
          if (error instanceof WebhookVerificationError) {
            return new Response('invalid signature', { status: 403 })
          }
          throw error
        }

        switch (event.type) {
          case 'order.paid':
          case 'subscription.created':
          case 'subscription.active':
          case 'subscription.updated':
          case 'subscription.canceled':
          case 'subscription.uncanceled':
          case 'subscription.revoked':
            // TODO(M7): forward to crates/server so workspace quotas and
            // access follow the subscription state.
            console.log(`[billing] ${event.type} #${event.data.id}`, event.data.metadata ?? {})
            break
          default:
            console.log(`[billing] ignoring event: ${event.type}`)
        }

        return Response.json({ received: true })
      },
    },
  },
})
