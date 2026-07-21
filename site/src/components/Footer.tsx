import { Link } from '@tanstack/react-router'

const COLUMNS = [
  {
    title: 'Product',
    links: [
      { label: 'Features', href: '/features' },
      { label: 'Cloud', href: '/cloud' },
      { label: 'Pricing', href: '/pricing' },
      {
        label: 'Download',
        href: 'https://github.com/simnJS/stake-dev-tool/releases/latest',
      },
      { label: 'Stake Engine', href: 'https://stake-engine.com/' },
    ],
  },
  {
    title: 'Project',
    links: [
      { label: 'Open source', href: '/open-source' },
      { label: 'GitHub', href: 'https://github.com/simnJS/stake-dev-tool' },
      {
        label: 'Issues',
        href: 'https://github.com/simnJS/stake-dev-tool/issues',
      },
      {
        label: 'Contributing',
        href: 'https://github.com/simnJS/stake-dev-tool/blob/main/CONTRIBUTING.md',
      },
      {
        label: 'Changelog',
        href: 'https://github.com/simnJS/stake-dev-tool/blob/main/CHANGELOG.md',
      },
    ],
  },
]

function FooterLink({ label, href }: { label: string; href: string }) {
  const className = 'text-sm text-moss no-underline transition-colors hover:text-ink'
  if (href.startsWith('/')) {
    return (
      <Link to={href} className={className}>
        {label}
      </Link>
    )
  }
  return (
    <a href={href} target="_blank" rel="noopener noreferrer" className={className}>
      {label}
    </a>
  )
}

export default function Footer() {
  return (
    <footer className="mt-28 border-t border-line">
      <div className="wrap grid gap-10 py-14 sm:grid-cols-[1.4fr_1fr_1fr]">
        <div>
          <div className="flex items-center gap-2.5">
            <img src="/icon-32.png" alt="" className="h-6 w-6" />
            <span className="display text-base font-bold tracking-tight">Stake Dev Tool</span>
          </div>
          <p className="mt-4 max-w-xs text-sm leading-relaxed text-moss">
            The open-source workbench for slot games on the Stake Engine RGS
            contract.
          </p>
        </div>

        {COLUMNS.map((col) => (
          <div key={col.title}>
            <h3 className="font-mono text-xs tracking-[0.14em] text-faint uppercase">
              {col.title}
            </h3>
            <ul className="mt-4 space-y-2.5 pl-0" style={{ listStyle: 'none' }}>
              {col.links.map((link) => (
                <li key={link.label}>
                  <FooterLink label={link.label} href={link.href} />
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>

      <div className="border-t border-line">
        <div className="wrap flex flex-col gap-3 py-6 font-mono text-xs text-faint sm:flex-row sm:items-center sm:justify-between">
          <span>© {new Date().getFullYear()} simnJS &amp; contributors</span>
          <nav className="flex flex-wrap gap-x-4 gap-y-1">
            {[
              { to: '/legal', label: 'Legal' },
              { to: '/terms', label: 'Terms' },
              { to: '/privacy', label: 'Privacy' },
              { to: '/refunds', label: 'Refunds' },
            ].map((item) => (
              <Link
                key={item.to}
                to={item.to}
                className="text-faint no-underline transition-colors hover:text-ink"
              >
                {item.label}
              </Link>
            ))}
          </nav>
          <span>Desktop &amp; engine: MIT · Cloud server: AGPL-3.0</span>
        </div>
      </div>
    </footer>
  )
}
