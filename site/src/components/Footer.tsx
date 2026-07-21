const COLUMNS = [
  {
    title: 'Product',
    links: [
      {
        label: 'Download',
        href: 'https://github.com/simnJS/stake-dev-tool/releases/latest',
      },
      {
        label: 'All releases',
        href: 'https://github.com/simnJS/stake-dev-tool/releases',
      },
      {
        label: 'V2 plan',
        href: 'https://github.com/simnJS/stake-dev-tool/blob/v2/V2.md',
      },
      { label: 'Stake Engine', href: 'https://stake-engine.com/' },
    ],
  },
  {
    title: 'Project',
    links: [
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
                  <a
                    href={link.href}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-sm text-moss no-underline transition-colors hover:text-ink"
                  >
                    {link.label}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>

      <div className="border-t border-line">
        <div className="wrap flex flex-col gap-2 py-6 font-mono text-xs text-faint sm:flex-row sm:items-center sm:justify-between">
          <span>© {new Date().getFullYear()} simnJS &amp; contributors</span>
          <span>Desktop &amp; engine: MIT · Cloud server (V2): AGPL-3.0</span>
        </div>
      </div>
    </footer>
  )
}
