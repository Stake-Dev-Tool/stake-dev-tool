import { Link, createFileRoute } from '@tanstack/react-router'
import LegalPage from '../components/LegalPage'
import { CONTACT_EMAIL, OPERATOR } from '../lib/site'

export const Route = createFileRoute('/privacy')({
  head: () => ({
    meta: [
      { title: 'Privacy policy — Stake Dev Tool' },
      {
        name: 'description',
        content: 'How Stake Dev Tool collects, uses and protects personal data.',
      },
    ],
  }),
  component: PrivacyPage,
})

function PrivacyPage() {
  return (
    <LegalPage title="Privacy policy" updated="21 July 2026">
      <p>
        This policy explains what personal data the Stake Dev Tool site and
        cloud service process, why, and what your rights are. The data
        controller is {OPERATOR.name} (see the{' '}
        <Link to="/legal">legal notice</Link>), reachable at{' '}
        <a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a>.
      </p>

      <h2>What we collect</h2>
      <ul>
        <li>
          <strong>Account data</strong>: email address, display name, and a
          hashed password (or your GitHub identity if you sign in with GitHub).
        </li>
        <li>
          <strong>Workspace content</strong>: the game bundles, math files,
          saved rounds and settings you upload to your workspaces.
        </li>
        <li>
          <strong>Billing status</strong>: your subscription plan and its
          state. Payment details (card numbers, billing address) are collected
          and stored by Stripe, our merchant of record, never by us.
        </li>
        <li>
          <strong>Technical logs</strong>: IP address, user agent and request
          metadata, kept briefly for security, debugging and rate limiting.
        </li>
        <li>
          <strong>Usage analytics</strong>: product analytics on the site and
          dashboard, and aggregate statistics on share links (sessions, spins,
          observed RTP). Share-link visitors are anonymous; we do not build
          profiles of them.
        </li>
      </ul>

      <h2>Why we process it</h2>
      <ul>
        <li>
          To provide the Service you signed up for, including hosting your
          content and syncing it across your team (performance of a contract).
        </li>
        <li>
          To keep the Service secure, prevent abuse, and understand how it is
          used so we can improve it (legitimate interest).
        </li>
        <li>To comply with legal obligations such as accounting.</li>
      </ul>
      <p>We do not sell personal data and we do not run advertising.</p>

      <h2>Who processes it for us</h2>
      <ul>
        <li>
          <strong>netcup GmbH</strong> (Germany): server hosting.
        </li>
        <li>
          <strong>Cloudflare</strong>: DNS and object storage for math blobs
          and game bundles.
        </li>
        <li>
          <strong>Stripe, Inc.</strong> (United States): payments, as
          merchant of record.
        </li>
        <li>
          <strong>Resend</strong>: transactional email (invites, account
          messages).
        </li>
        <li>
          <strong>PostHog</strong> (EU hosting): product analytics.
        </li>
      </ul>
      <p>
        Data is stored in the European Union wherever possible. Where a
        processor is outside the EU (such as Stripe), transfers rely on
        recognised safeguards such as standard contractual clauses.
      </p>

      <h2>How long we keep it</h2>
      <ul>
        <li>Account data and workspace content: while your account is active.</li>
        <li>
          After deletion: removed from production immediately and from backups
          on their normal rotation schedule.
        </li>
        <li>Technical logs: a few weeks at most.</li>
        <li>
          Billing records: kept by Stripe and by us as required by tax law.
        </li>
      </ul>

      <h2>Cookies</h2>
      <p>
        The dashboard uses an essential session cookie to keep you signed in.
        Analytics on the public site are configured without cross-site
        tracking. We do not use advertising cookies.
      </p>

      <h2>Your rights</h2>
      <p>
        Under the GDPR you can request access to your data, its rectification,
        deletion or portability, and you can restrict or object to certain
        processing. Write to{' '}
        <a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a> and we will
        respond within a month. You can also lodge a complaint with the French
        supervisory authority, the CNIL (
        <a href="https://www.cnil.fr" target="_blank" rel="noopener noreferrer">
          cnil.fr
        </a>
        ).
      </p>

      <h2>Changes</h2>
      <p>
        If this policy changes materially, we will notify account holders by
        email or in the dashboard.
      </p>
    </LegalPage>
  )
}
