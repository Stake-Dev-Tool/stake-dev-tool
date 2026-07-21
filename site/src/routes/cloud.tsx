import { Link, createFileRoute } from '@tanstack/react-router'
import SectionRule from '../components/SectionRule'

export const Route = createFileRoute('/cloud')({
  head: () => ({
    meta: [
      { title: 'Cloud — Stake Dev Tool' },
      {
        name: 'description',
        content:
          'The web workbench and real hosted share links. One Rust engine, three surfaces: desktop, browser, and hosted game instances on their own subdomains.',
      },
    ],
  }),
  component: CloudPage,
})

const SURFACES = [
  {
    title: 'Desktop app',
    desc: 'The local dev loop stays king: front hot-reload, local math, instant restarts. Free and MIT forever.',
  },
  {
    title: 'Web workbench',
    desc: 'The same test view, served from the cloud. Math devs, QA and PMs get the full workbench with zero install.',
  },
  {
    title: 'Share links',
    desc: 'Each link is a real hosted game instance on its own subdomain, not a static export. Testers open the URL and play against a real server-side RGS.',
  },
]

const POINTS = [
  {
    title: 'Math never leaves the server',
    desc: 'Share links run full fidelity without shipping your books to the browser. Full privacy and full fidelity at the same time.',
  },
  {
    title: 'Replays stay valid forever',
    desc: 'Saved rounds reference (revision, mode, eventId). Push new math whenever you want; old bookmarks keep replaying exactly as recorded.',
  },
  {
    title: 'Auto changelog between revisions',
    desc: 'The server computes bet-stats per revision: RTP per mode, max win, modes added or removed. Every push gets a diff.',
  },
  {
    title: 'Link controls',
    desc: 'Pin a link to a fixed revision or track latest. Set an expiry, add a password, share a replay of one exact round.',
  },
  {
    title: 'Per-link analytics',
    desc: 'Sessions, spins and observed RTP for every share link, so you know who actually played and what they hit.',
  },
  {
    title: 'Origin isolation',
    desc: 'Shared games live on their own subdomains, strictly separated from the dashboard, so workspace cookies can never leak into a game.',
  },
]

function CloudPage() {
  return (
    <main className="wrap pt-16 pb-8">
      <SectionRule label="crates/server" />
      <h1 className="display mt-12 mb-0 max-w-2xl text-4xl font-bold sm:text-5xl">
        One engine, three surfaces.
      </h1>
      <p className="mt-6 mb-0 max-w-xl leading-relaxed text-moss">
        The same Rust engine that powers your local loop also runs in the
        cloud: a web workbench for your team, and real hosted game instances
        behind every share link.
      </p>

      <div className="mt-14 grid gap-4 sm:grid-cols-3">
        {SURFACES.map((surface, i) => (
          <article key={surface.title} className="card p-6">
            <span className="font-mono text-xs text-mint">{`0${i + 1}`}</span>
            <h2 className="mt-2 mb-0 text-[1.02rem] font-semibold">{surface.title}</h2>
            <p className="mt-2.5 mb-0 text-sm leading-relaxed text-moss">{surface.desc}</p>
          </article>
        ))}
      </div>

      <div className="card mt-10 overflow-x-auto p-6">
        <p className="m-0 font-mono text-[0.78rem] leading-relaxed whitespace-pre text-dim">
          <span className="text-mint">https://big-bass-frenzy.play.yourdomain.com</span>
          {'\n'}
          {'├── /             '}
          <span className="text-faint">your game front bundle</span>
          {'\n'}
          {'└── /api/rgs/...  '}
          <span className="text-faint">server-side RGS · workspace math · pinned to rev 42</span>
        </p>
      </div>

      <div className="mt-10 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {POINTS.map((point) => (
          <article key={point.title} className="card card-hover p-6">
            <h2 className="m-0 text-[0.95rem] font-semibold">{point.title}</h2>
            <p className="mt-2 mb-0 text-sm leading-relaxed text-moss">{point.desc}</p>
          </article>
        ))}
      </div>

      <div className="mt-14 flex flex-wrap gap-3">
        <Link to="/pricing" className="btn btn-primary">
          Start a free trial
        </Link>
        <a
          href="https://github.com/simnJS/stake-dev-tool/blob/v2/V2.md"
          target="_blank"
          rel="noopener noreferrer"
          className="btn btn-ghost"
        >
          Read the architecture plan
        </a>
      </div>
    </main>
  )
}
