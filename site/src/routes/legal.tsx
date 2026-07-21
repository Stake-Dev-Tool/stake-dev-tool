import { createFileRoute } from '@tanstack/react-router'
import LegalPage from '../components/LegalPage'
import { CONTACT_EMAIL, OPERATOR } from '../lib/site'

export const Route = createFileRoute('/legal')({
  head: () => ({
    meta: [
      { title: 'Legal notice — Stake Dev Tool' },
      { name: 'description', content: 'Legal notice (mentions légales) for stakedevtool.com.' },
    ],
  }),
  component: LegalNoticePage,
})

function LegalNoticePage() {
  return (
    <LegalPage title="Legal notice" updated="21 July 2026">
      <h2>Site operator</h2>
      <p>
        This website and the Stake Dev Tool cloud service are operated by{' '}
        <strong>{OPERATOR.name}</strong>, {OPERATOR.status}.
      </p>
      <ul>
        <li>SIREN: {OPERATOR.siren}</li>
        <li>SIRET (head office): {OPERATOR.siret}</li>
        <li>EU VAT number: {OPERATOR.vat}</li>
        <li>Registered office: {OPERATOR.address}</li>
        <li>Publication director: {OPERATOR.name}</li>
        <li>
          Contact: <a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a>
        </li>
      </ul>

      <h2>Hosting</h2>
      <p>
        The site and service are hosted by <strong>netcup GmbH</strong>,
        Emmy-Noether-Str. 10, 76131 Karlsruhe, Germany (
        <a href="https://www.netcup.com" target="_blank" rel="noopener noreferrer">
          netcup.com
        </a>
        ).
      </p>

      <h2>Payments</h2>
      <p>
        Paid subscriptions are sold by <strong>Polar Software Inc.</strong> acting
        as merchant of record. Polar appears on your invoice and handles payment
        processing and VAT remittance.
      </p>

      <h2>Source code</h2>
      <p>
        The Stake Dev Tool platform is open source: the desktop app and engine
        are released under the MIT licence and the cloud server under the
        AGPL-3.0 licence, at{' '}
        <a
          href="https://github.com/simnJS/stake-dev-tool"
          target="_blank"
          rel="noopener noreferrer"
        >
          github.com/simnJS/stake-dev-tool
        </a>
        .
      </p>

      <h2>Trademarks</h2>
      <p>
        Stake Engine is a product of its respective owner. Stake Dev Tool is an
        independent developer tool and is not affiliated with, endorsed by, or
        sponsored by Stake.
      </p>
    </LegalPage>
  )
}
