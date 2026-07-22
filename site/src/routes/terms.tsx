import { Link, createFileRoute } from '@tanstack/react-router'
import LegalPage from '../components/LegalPage'
import { CONTACT_EMAIL, OPERATOR } from '../lib/site'

export const Route = createFileRoute('/terms')({
  head: () => ({
    meta: [
      { title: 'Terms of Service — Stake Dev Tool' },
      {
        name: 'description',
        content: 'Terms of Service for the Stake Dev Tool hosted cloud service.',
      },
    ],
  }),
  component: TermsPage,
})

function TermsPage() {
  return (
    <LegalPage title="Terms of Service" updated="21 July 2026">
      <p>
        These terms govern your use of the Stake Dev Tool hosted cloud service
        at stakedevtool.com (the "Service"), operated by {OPERATOR.name} (see
        the <Link to="/legal">legal notice</Link>). By creating an account or
        subscribing, you agree to them.
      </p>
      <p>
        The open-source Stake Dev Tool software itself is not covered by these
        terms: it is licensed under MIT and AGPL-3.0, and you can self-host it
        without any relationship with us.
      </p>

      <h2>1. The Service</h2>
      <p>
        The Service is a hosted developer platform for slot games built on the
        Stake Engine RGS contract: a web workbench, versioned math storage,
        team workspaces, and shareable hosted game instances. It is a
        business-to-business developer tool.
      </p>
      <p>
        The Service is not gambling. Games running on the Service use fictional
        balances only; no wagers are placed, no money can be won, and no
        real-money gambling of any kind takes place on the platform.
      </p>

      <h2>2. Accounts</h2>
      <p>
        You must provide accurate information when creating an account and keep
        your credentials secure. You are responsible for activity in your
        account and in workspaces you administer. You must be at least 18 years
        old.
      </p>

      <h2>3. Subscriptions and billing</h2>
      <ul>
        <li>
          Paid plans are processed by Stripe, Inc. acting as merchant of
          record. Stripe's own terms apply to the purchase itself, and Stripe
          handles payment processing and VAT.
        </li>
        <li>
          There is no free trial on the hosted service: billing begins when you
          subscribe, and until then a hosted workspace is read-only. If you would
          rather not pay, the entire platform is open source and free to
          self-host, with every feature included.
        </li>
        <li>
          Subscriptions renew automatically at the end of each billing period.
          You can cancel at any time from the billing portal; cancellation
          takes effect at the end of the current period.
        </li>
        <li>
          Refunds are handled as described in the{' '}
          <Link to="/refunds">refund policy</Link>.
        </li>
        <li>
          Plan quotas (storage, members, share sessions) are described on the{' '}
          <Link to="/pricing">pricing page</Link>. We may enforce fair-use
          limits such as rate limiting to keep the Service healthy for
          everyone.
        </li>
      </ul>

      <h2>4. Your content</h2>
      <p>
        You keep full ownership of everything you upload: game bundles, math
        files, saved rounds and other workspace content. You grant us only the
        rights needed to operate the Service: storing your content, serving it
        to your workspace members, and serving game front bundles to people you
        share links with. Share links are designed so that your math files are
        never distributed to visitors.
      </p>
      <p>
        You are responsible for having the rights to the content you upload,
        and for that content complying with applicable law.
      </p>

      <h2>5. Acceptable use</h2>
      <ul>
        <li>No unlawful use, and no uploading of infringing or malicious content.</li>
        <li>
          No operating real-money gambling through the Service, and no
          presenting hosted game instances to the public as real gambling.
        </li>
        <li>
          No attempts to break, overload or probe the Service's security, and
          no circumventing plan quotas.
        </li>
        <li>
          No reselling the Service itself. (Self-hosting the open-source
          platform, including commercially, is of course fine and governed by
          the code licences, not by these terms.)
        </li>
      </ul>

      <h2>6. Availability and support</h2>
      <p>
        We aim for the Service to be reliable and we back up its data, but it
        is provided without an uptime guarantee or service-level agreement.
        Support is provided on a reasonable-efforts basis at{' '}
        <a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a>.
      </p>

      <h2>7. Suspension and termination</h2>
      <p>
        You can stop using the Service and delete your account at any time. We
        may suspend or terminate accounts that breach these terms, abuse the
        Service, or have unpaid subscriptions, with reasonable notice where
        practicable. After account deletion, your content is removed from
        production systems and then from backups on their normal rotation
        schedule.
      </p>
      <p>
        Because the platform is open source, ending your subscription never
        locks you out of your workflow: you can export your content and
        self-host the same software.
      </p>

      <h2>8. Warranty and liability</h2>
      <p>
        The Service is provided "as is". To the extent permitted by law, we
        exclude implied warranties, and our total liability for claims arising
        from the Service is limited to the amount you paid for it in the twelve
        months preceding the claim. Nothing in these terms excludes liability
        that cannot be excluded by law, or affects mandatory consumer rights
        where they apply.
      </p>

      <h2>9. Changes</h2>
      <p>
        We may update these terms as the Service evolves. For material changes,
        we will notify account holders by email or in the dashboard before the
        changes take effect. Continued use after that constitutes acceptance.
      </p>

      <h2>10. Governing law</h2>
      <p>
        These terms are governed by French law, without prejudice to any
        mandatory protections of the country where you live. Disputes that
        cannot be resolved amicably fall under the jurisdiction of the French
        courts.
      </p>

      <h2>Contact</h2>
      <p>
        Questions about these terms:{' '}
        <a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a>.
      </p>
    </LegalPage>
  )
}
