import { Link, createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/checkout/cancelled')({
  head: () => ({
    meta: [{ title: 'Checkout cancelled — Stake Dev Tool' }],
  }),
  component: CheckoutCancelledPage,
})

function CheckoutCancelledPage() {
  return (
    <main className="wrap flex min-h-[55vh] items-center justify-center py-20">
      <div className="card max-w-md p-10 text-center">
        <p className="m-0 font-mono text-xs tracking-[0.16em] text-moss uppercase">
          Checkout cancelled
        </p>
        <h1 className="display mt-4 mb-0 text-3xl font-bold">Nothing was charged.</h1>
        <p className="mt-4 mb-0 text-sm leading-relaxed text-moss">
          The checkout was closed before payment. Your plan choice is still
          waiting whenever you want to pick it back up.
        </p>
        <Link to="/pricing" className="btn btn-ghost mt-8">
          Back to pricing
        </Link>
      </div>
    </main>
  )
}
