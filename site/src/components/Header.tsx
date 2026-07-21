import { Link } from '@tanstack/react-router'

const NAV = [
  { to: '/features', label: 'Features' },
  { to: '/cloud', label: 'Cloud' },
  { to: '/pricing', label: 'Pricing' },
  { to: '/open-source', label: 'Open source' },
] as const

export default function Header() {
  return (
    <header className="sticky top-0 z-40 border-b border-line bg-pit/85 backdrop-blur-md">
      <div className="wrap flex flex-wrap items-center justify-between gap-x-4 gap-y-0 py-3 md:h-16 md:py-0">
        <Link to="/" className="flex items-center gap-2.5 no-underline">
          <img src="/icon-32.png" alt="" className="h-6 w-6" />
          <span className="display text-[1.02rem] font-bold tracking-tight">
            Stake Dev Tool
          </span>
        </Link>

        <nav className="order-3 -mb-1 flex w-full items-center gap-6 overflow-x-auto pt-2 pb-2 md:order-none md:w-auto md:gap-7 md:pt-0 md:pb-0">
          {NAV.map((item) => (
            <Link
              key={item.to}
              to={item.to}
              className="text-sm whitespace-nowrap text-moss no-underline transition-colors hover:text-ink"
              activeProps={{ className: 'text-ink' }}
            >
              {item.label}
            </Link>
          ))}
        </nav>

        <div className="flex items-center gap-3">
          <a
            href="https://github.com/simnJS/stake-dev-tool"
            target="_blank"
            rel="noopener noreferrer"
            aria-label="GitHub repository"
            className="text-moss transition-colors hover:text-ink"
          >
            <svg viewBox="0 0 16 16" width="20" height="20" fill="currentColor" aria-hidden="true">
              <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
            </svg>
          </a>
          <a
            href="https://app.stakedevtool.com"
            className="btn btn-ghost hidden !px-4 !py-2.5 text-sm sm:inline-flex"
          >
            Dashboard
          </a>
          <a
            href="https://github.com/simnJS/stake-dev-tool/releases/latest"
            target="_blank"
            rel="noopener noreferrer"
            className="btn btn-primary hidden !px-4 !py-2.5 text-sm sm:inline-flex"
          >
            Download
          </a>
        </div>
      </div>
    </header>
  )
}
