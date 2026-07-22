import { Link, createFileRoute } from '@tanstack/react-router'
import LegalPage from '../components/LegalPage'
import { CONTACT_EMAIL } from '../lib/site'

export const Route = createFileRoute('/refunds')({
  head: () => ({
    meta: [
      { title: 'Refund policy — Stake Dev Tool' },
      {
        name: 'description',
        content: 'Cancellation and refund terms for Stake Dev Tool cloud plans.',
      },
    ],
  }),
  component: RefundsPage,
})

function RefundsPage() {
  return (
    <LegalPage title="Refund policy" updated="21 July 2026">
      <h2>Evaluate without risk</h2>
      <p>
        There is no free trial on the hosted service, but you take on no risk.
        The entire platform is open source, so you can self-host it for free and
        try every feature before you ever pay. And if you subscribe to the hosted
        service and it does not work out, the first-payment guarantee below
        refunds you in full.
      </p>

      <h2>First payment</h2>
      <p>
        If you subscribed and the Service does not work out, contact us at{' '}
        <a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a> within 14 days
        of your first charge and we will refund it in full, no questions asked.
      </p>

      <h2>Renewals</h2>
      <p>
        You can cancel at any time from the billing portal; cancellation takes
        effect at the end of the current billing period and no further renewals
        are charged. Renewal charges themselves are not refundable, except
        where the law requires otherwise or in cases like duplicate charges or
        billing errors, which we will always fix.
      </p>

      <h2>How refunds are paid</h2>
      <p>
        Payments are processed by Stripe, our merchant of record, so approved
        refunds are issued by Stripe to your original payment method. Refunds
        usually appear within 5 to 10 business days depending on your bank.
      </p>

      <h2>Self-hosting is always free</h2>
      <p>
        If the hosted service is not for you, the entire platform is open
        source and free to <Link to="/open-source">self-host</Link>, with every
        feature included.
      </p>
    </LegalPage>
  )
}
