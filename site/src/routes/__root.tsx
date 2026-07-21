import { HeadContent, Scripts, createRootRoute } from '@tanstack/react-router'
import { TanStackRouterDevtoolsPanel } from '@tanstack/react-router-devtools'
import { TanStackDevtools } from '@tanstack/react-devtools'
import Footer from '../components/Footer'
import Header from '../components/Header'

import appCss from '../styles.css?url'

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      { title: 'Stake Dev Tool — the workbench for Stake Engine slots' },
      {
        name: 'description',
        content:
          'Open-source workbench for slot games on the Stake Engine RGS contract. Run your game against a fast Rust RGS, QA it at every resolution, replay any round, and share real playable links. Self-host free forever.',
      },
      { property: 'og:title', content: 'Stake Dev Tool' },
      {
        property: 'og:description',
        content:
          'Run, debug and QA slot games on the Stake Engine RGS contract. Open source, self-hostable, with an optional hosted cloud.',
      },
      { property: 'og:type', content: 'website' },
      { name: 'theme-color', content: '#0a100d' },
    ],
    links: [
      { rel: 'stylesheet', href: appCss },
      { rel: 'icon', href: '/favicon.ico', sizes: 'any' },
      { rel: 'icon', type: 'image/png', sizes: '32x32', href: '/icon-32.png' },
      { rel: 'apple-touch-icon', href: '/icon-128.png' },
    ],
  }),
  shellComponent: RootDocument,
})

function RootDocument({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <head>
        <HeadContent />
      </head>
      <body className="bg-pit font-sans text-ink antialiased">
        <Header />
        {children}
        <Footer />
        {import.meta.env.DEV ? (
          <TanStackDevtools
            config={{ position: 'bottom-right' }}
            plugins={[
              {
                name: 'Tanstack Router',
                render: <TanStackRouterDevtoolsPanel />,
              },
            ]}
          />
        ) : null}
        <Scripts />
      </body>
    </html>
  )
}
