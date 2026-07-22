import { Link, createFileRoute } from '@tanstack/react-router'
import SectionRule from '../components/SectionRule'
import TestViewFigure from '../components/TestViewFigure'

export const Route = createFileRoute('/')({ component: Home })

const REPO = 'https://github.com/simnJS/stake-dev-tool'

const HIGHLIGHTS = [
  {
    title: 'Fast Rust LGS',
    desc: 'A drop-in RGS server on your machine. It reads your math files straight from disk and answers wallet calls in microseconds.',
  },
  {
    title: 'Multi-resolution test view',
    desc: 'The same game at seven resolutions side by side, each frame its own session. QA sees exactly what players will.',
  },
  {
    title: 'Real share links',
    desc: 'Hand anyone a URL on its own subdomain. They play against a real server-side RGS, and your math never leaves the server.',
  },
]

function Home() {
  return (
    <main>
      {/* Hero */}
      <section className="wrap grid items-center gap-14 pt-20 pb-24 lg:grid-cols-[1.05fr_1fr] lg:gap-12 lg:pt-28">
        <div>
          <p className="rise m-0 font-mono text-xs tracking-[0.16em] text-mint uppercase">
            Open-source workbench · Stake Engine RGS
          </p>
          <h1
            className="display rise mt-5 mb-0 text-[2.7rem] leading-[1.04] font-bold sm:text-[3.6rem]"
            style={{ animationDelay: '80ms' }}
          >
            Ship slots with a&nbsp;real dev&nbsp;loop.
          </h1>
          <p
            className="rise mt-6 mb-0 max-w-lg text-[1.05rem] leading-relaxed text-moss"
            style={{ animationDelay: '160ms' }}
          >
            Stake Dev Tool runs your game against a fast Rust RGS on your
            machine. Test it at every resolution side by side, replay any
            round, and hand your team a link that just plays.
          </p>
          <div className="rise mt-9 flex flex-wrap gap-3" style={{ animationDelay: '240ms' }}>
            <a
              href={`${REPO}/releases/latest`}
              target="_blank"
              rel="noopener noreferrer"
              className="btn btn-primary"
            >
              Download for desktop
            </a>
            <Link to="/pricing" className="btn btn-ghost">
              See cloud pricing
            </Link>
          </div>
          <p
            className="rise mt-7 mb-0 font-mono text-[0.7rem] tracking-[0.08em] text-faint"
            style={{ animationDelay: '320ms' }}
          >
            Windows · macOS · Linux · open source
          </p>
        </div>

        <TestViewFigure />
      </section>

      {/* Highlights */}
      <section className="wrap pt-4 pb-24">
        <SectionRule label="crates/lgs · ui/" />
        <h2 className="display mt-12 mb-0 max-w-xl text-3xl font-bold sm:text-4xl">
          Everything the RGS contract needs.
        </h2>
        <div className="mt-12 grid gap-4 sm:grid-cols-3">
          {HIGHLIGHTS.map((item) => (
            <article key={item.title} className="card p-6">
              <h3 className="m-0 text-[1.02rem] font-semibold">{item.title}</h3>
              <p className="mt-2.5 mb-0 text-sm leading-relaxed text-moss">{item.desc}</p>
            </article>
          ))}
        </div>
        <Link
          to="/features"
          className="mt-8 inline-block font-mono text-sm text-mint no-underline hover:underline"
        >
          All features →
        </Link>
      </section>

      {/* Cloud teaser */}
      <section className="wrap pt-4 pb-24">
        <SectionRule label="crates/server" />
        <div className="mt-12 grid items-center gap-10 lg:grid-cols-[1fr_1fr]">
          <div>
            <h2 className="display mt-0 mb-0 text-3xl font-bold sm:text-4xl">
              One engine, three surfaces.
            </h2>
            <p className="mt-5 mb-0 leading-relaxed text-moss">
              The same Rust engine that powers your local loop also runs in the
              cloud: a web workbench for your team, and real hosted game
              instances behind every share link. Zero install for math devs,
              QA and PMs.
            </p>
            <Link to="/cloud" className="btn btn-ghost mt-7">
              Explore the cloud platform
            </Link>
          </div>
          <div className="card overflow-x-auto p-6">
            <p className="m-0 font-mono text-[0.78rem] leading-relaxed whitespace-pre text-dim">
              <span className="text-mint">https://big-bass-frenzy.play.yourdomain.com</span>
              {'\n'}
              {'├── /             '}
              <span className="text-faint">your game front bundle</span>
              {'\n'}
              {'└── /api/rgs/...  '}
              <span className="text-faint">server-side RGS · pinned to rev 42</span>
            </p>
          </div>
        </div>
      </section>

      {/* Final CTA */}
      <section className="wrap pt-8 pb-4">
        <div className="card relative overflow-hidden p-10 text-center sm:p-14">
          <div
            className="pointer-events-none absolute inset-0"
            style={{
              background:
                'radial-gradient(560px 260px at 50% 0%, rgba(52, 211, 153, 0.1), transparent 70%)',
            }}
          />
          <h2 className="display relative mt-0 mb-0 text-3xl font-bold sm:text-4xl">
            Your next slot deserves a real dev loop.
          </h2>
          <p className="relative mx-auto mt-4 mb-0 max-w-md leading-relaxed text-moss">
            Download the desktop app and point it at your game, or bring your
            whole team onto the cloud workbench.
          </p>
          <div className="relative mt-8 flex flex-wrap justify-center gap-3">
            <a
              href={`${REPO}/releases/latest`}
              target="_blank"
              rel="noopener noreferrer"
              className="btn btn-primary"
            >
              Download for desktop
            </a>
            <Link to="/pricing" className="btn btn-ghost">
              Get started
            </Link>
          </div>
        </div>
      </section>
    </main>
  )
}
