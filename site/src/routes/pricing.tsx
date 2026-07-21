import { useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useServerFn } from '@tanstack/react-start'
import SectionRule from '../components/SectionRule'
import { PLANS, SELF_HOST_FEATURES } from '../lib/plans'
import type { BillingInterval, PlanId } from '../lib/plans'
import { createCheckout } from '../server/billing'

export const Route = createFileRoute('/pricing')({
  head: () => ({
    meta: [
      { title: 'Pricing — Stake Dev Tool' },
      {
        name: 'description',
        content:
          'Self-hosting is free forever with every feature. The cloud subscription sells hosting: Solo €5/month, Team €15/month, 14-day free trial.',
      },
    ],
  }),
  component: PricingPage,
})

const REPO = 'https://github.com/simnJS/stake-dev-tool'

const FAQ = [
  {
    q: 'Is the paid version different from the open-source one?',
    a: 'No. The entire platform is open source and self-hosters get 100% of the features. There is no enterprise edition and no feature gating. Billing code only runs on our hosted instance.',
  },
  {
    q: 'What exactly does the subscription pay for?',
    a: 'Hosting. Zero-install access, wildcard play subdomains, storage, backups and updates on our infrastructure. If you would rather run it yourself, everything is in the repo.',
  },
  {
    q: 'What do I need to self-host the cloud platform?',
    a: 'A single server binary (or Docker image), Postgres, and Caddy for TLS. Object storage is the local filesystem by default, or any S3-compatible bucket.',
  },
  {
    q: 'How does the trial work?',
    a: 'Every cloud plan starts with a 14-day free trial: you are not charged until the trial ends, and you can cancel from the billing portal at any time. Payments and EU VAT are handled by our merchant of record.',
  },
  {
    q: 'Why AGPL for the server?',
    a: 'Self-hosting is untouched. The AGPL only prevents someone from reselling our server as a closed hosted service, the same licence choice Plausible and Cal.com made. The desktop app and the engine stay MIT.',
  },
]

function PlanCard({
  plan,
  interval,
  onSubscribe,
  pending,
  highlighted,
}: {
  plan: PlanId
  interval: BillingInterval
  onSubscribe: (plan: PlanId) => void
  pending: boolean
  highlighted?: boolean
}) {
  const def = PLANS[plan]
  const price = interval === 'month' ? def.monthly : def.yearly
  return (
    <article className={`card flex flex-col p-7 ${highlighted ? 'border-amber/35' : ''}`}>
      <div className="flex items-center justify-between gap-3">
        <h2 className="m-0 text-base font-semibold">{def.name}</h2>
        <span
          className={`rounded-full border px-2.5 py-1 font-mono text-[0.62rem] tracking-[0.1em] uppercase ${
            highlighted ? 'border-amber/40 text-amber' : 'border-line2 text-moss'
          }`}
        >
          Cloud
        </span>
      </div>
      <p className="display mt-4 mb-0 text-4xl font-bold">
        €{price}
        <span className="font-sans ml-2 text-sm font-normal text-faint">
          / {interval === 'month' ? 'month' : 'year'}
        </span>
      </p>
      <ul className="mt-6 mb-0 flex-1 space-y-2.5 pl-0 text-sm text-moss" style={{ listStyle: 'none' }}>
        {def.features.map((feature) => (
          <li key={feature}>{feature}</li>
        ))}
      </ul>
      <button
        type="button"
        onClick={() => onSubscribe(plan)}
        disabled={pending}
        className="btn btn-primary mt-7 w-full disabled:cursor-wait disabled:opacity-60"
      >
        {pending ? 'Opening checkout…' : 'Start 14-day trial'}
      </button>
    </article>
  )
}

function PricingPage() {
  const checkout = useServerFn(createCheckout)
  const [interval, setInterval] = useState<BillingInterval>('month')
  const [pendingPlan, setPendingPlan] = useState<PlanId | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function subscribe(plan: PlanId) {
    setError(null)
    setPendingPlan(plan)
    try {
      const result = await checkout({ data: { plan, interval } })
      if (result.url) {
        window.location.assign(result.url)
        return
      }
      setError(
        result.reason === 'not_configured'
          ? 'Checkout is not available yet on this deployment.'
          : 'The payment provider did not respond. Try again in a minute.',
      )
    } catch {
      setError('Something went wrong starting the checkout. Try again in a minute.')
    } finally {
      setPendingPlan(null)
    }
  }

  return (
    <main className="wrap pt-16 pb-8">
      <SectionRule label="pricing" />
      <h1 className="display mt-12 mb-0 max-w-xl text-4xl font-bold sm:text-5xl">
        Self-hosting is free. Forever.
      </h1>
      <p className="mt-6 mb-0 max-w-2xl leading-relaxed text-moss">
        The whole platform is open source; the subscription sells hosting and
        nothing else: zero-install access, wildcard play subdomains, storage,
        backups and updates. There is no feature gating and no enterprise
        edition.
      </p>

      {/* Interval toggle */}
      <div className="mt-10 inline-flex items-center rounded-lg border border-line p-1">
        {(['month', 'year'] as const).map((value) => (
          <button
            key={value}
            type="button"
            onClick={() => setInterval(value)}
            aria-pressed={interval === value}
            className={`rounded-md px-4 py-2 text-sm font-medium transition-colors ${
              interval === value ? 'bg-panel2 text-ink' : 'text-moss hover:text-ink'
            }`}
          >
            {value === 'month' ? 'Monthly' : 'Yearly'}
          </button>
        ))}
        <span className="px-3 font-mono text-[0.65rem] tracking-[0.08em] text-amber">
          2 months free
        </span>
      </div>

      <div className="mt-8 grid gap-4 lg:grid-cols-3">
        {/* Self-host */}
        <article className="card flex flex-col border-dashed p-7">
          <h2 className="m-0 text-base font-semibold">Self-host</h2>
          <p className="display mt-4 mb-0 text-4xl font-bold">
            €0
            <span className="font-sans ml-2 text-sm font-normal text-faint">forever</span>
          </p>
          <ul className="mt-6 mb-0 flex-1 space-y-2.5 pl-0 text-sm text-moss" style={{ listStyle: 'none' }}>
            {SELF_HOST_FEATURES.map((feature) => (
              <li key={feature}>{feature}</li>
            ))}
          </ul>
          <a
            href={REPO}
            target="_blank"
            rel="noopener noreferrer"
            className="btn btn-ghost mt-7 w-full"
          >
            Deploy from GitHub
          </a>
        </article>

        <PlanCard
          plan="solo"
          interval={interval}
          onSubscribe={subscribe}
          pending={pendingPlan === 'solo'}
          highlighted
        />
        <PlanCard
          plan="team"
          interval={interval}
          onSubscribe={subscribe}
          pending={pendingPlan === 'team'}
        />
      </div>

      {error ? (
        <p className="mt-5 mb-0 font-mono text-[0.75rem] text-amber" role="alert">
          {error}
        </p>
      ) : null}

      <p className="mt-6 mb-0 font-mono text-[0.7rem] tracking-[0.06em] text-faint">
        14-day free trial · cancel anytime · payments and VAT handled by our merchant of record
      </p>

      {/* FAQ */}
      <section className="pt-20">
        <SectionRule label="FAQ" />
        <div className="mt-12 grid gap-12 lg:grid-cols-[1fr_1.4fr]">
          <h2 className="display mt-0 mb-0 text-3xl font-bold sm:text-4xl">Fair questions.</h2>
          <div>
            {FAQ.map((item) => (
              <details key={item.q} className="faq-item border-b border-line py-5 first:pt-0">
                <summary className="flex items-baseline justify-between gap-6 text-[0.98rem] font-medium">
                  {item.q}
                  <span className="faq-marker font-mono text-mint" aria-hidden="true" />
                </summary>
                <p className="mt-3 mb-0 max-w-xl text-sm leading-relaxed text-moss">{item.a}</p>
              </details>
            ))}
          </div>
        </div>
      </section>
    </main>
  )
}
