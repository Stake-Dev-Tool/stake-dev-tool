import { Link, createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/checkout/success')({
  head: () => ({
    meta: [{ title: 'Subscription confirmed — Stake Dev Tool' }],
  }),
  component: CheckoutSuccessPage,
})

function CheckoutSuccessPage() {
  return (
    <main className="wrap flex min-h-[55vh] items-center justify-center py-20">
      <div className="card max-w-md p-10 text-center">
        <p className="m-0 font-mono text-xs tracking-[0.16em] text-mint uppercase">
          Subscription confirmed
        </p>
        <h1 className="display mt-4 mb-0 text-3xl font-bold">You&apos;re in.</h1>
        <p className="mt-4 mb-0 text-sm leading-relaxed text-moss">
          Your receipt and next steps are in your inbox. Your workspace is
          ready as soon as you sign in.
        </p>
        <Link to="/" className="btn btn-primary mt-8">
          Back to the site
        </Link>
      </div>
    </main>
  )
}
