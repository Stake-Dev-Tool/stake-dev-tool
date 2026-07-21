import { createFileRoute } from '@tanstack/react-router'
import TestViewFigure from '../components/TestViewFigure'

export const Route = createFileRoute('/')({ component: Home })

const REPO = 'https://github.com/simnJS/stake-dev-tool'

const FEATURES = [
  {
    title: 'Fast Rust LGS',
    desc: 'A drop-in /api/rgs/<game>/wallet server. Reads index.json, lookup tables and books_*.jsonl.zst straight from disk. Books are indexed once per mode, with weighted RNG via binary search.',
  },
  {
    title: 'Multi-resolution test view',
    desc: 'Run your game side by side at seven built-in resolutions plus any custom size. Every frame is its own session, so QA sees exactly what players will.',
  },
  {
    title: 'Live event stream',
    desc: 'Server-sent events push every spin to the test view, with per-frame bet history and a last-event strip. The view updates the instant a spin lands.',
  },
  {
    title: 'Force, replay, bookmark',
    desc: 'Pin any (mode, eventId), replay a saved outcome, and keep notable rounds. Min, average and max wins are picked automatically for each mode.',
  },
  {
    title: 'Local HTTPS',
    desc: 'A bundled CA installs into your user trust store. Your game runs on https without browser warnings or manual certificate work.',
  },
  {
    title: 'Team sync',
    desc: 'Share profiles, saved rounds and bookmarks across your team. In V2, cloud workspaces replace repo-based sync entirely.',
  },
]

const SURFACES = [
  {
    title: 'Desktop app',
    desc: 'The local dev loop stays king: front hot-reload, local math, instant restarts. Unchanged in V2, and MIT forever.',
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

const CLOUD_POINTS = [
  {
    title: 'Math never leaves the server',
    desc: 'Share links run full fidelity without shipping your books to the browser. The old Sampled and Partial privacy modes simply disappear.',
  },
  {
    title: 'Replays stay valid forever',
    desc: 'Saved rounds reference (revision, mode, eventId). Push new math whenever you want; old bookmarks keep replaying exactly as recorded.',
  },
  {
    title: 'Auto changelog between revisions',
    desc: 'The server computes bet-stats per revision: RTP per mode, max win, modes added or removed. Every push gets a diff.',
  },
]

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
    q: 'I use the desktop app today — what changes?',
    a: 'Nothing breaks. The desktop app keeps its role as the local dev loop. GitHub-repo team sync will be marked deprecated once cloud workspaces land, with a migration path for existing teams.',
  },
  {
    q: 'Why AGPL for the server?',
    a: 'Self-hosting is untouched. The AGPL only prevents someone from reselling our server as a closed hosted service, the same licence choice Plausible and Cal.com made. The desktop app and the engine stay MIT.',
  },
]

function SectionRule({ label }: { label: string }) {
  return (
    <div className="section-rule">
      <span className="rule-label">{label}</span>
    </div>
  )
}

function Home() {
  return (
    <main>
      {/* ───────────────────────── Hero ───────────────────────── */}
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
            <a href={REPO} target="_blank" rel="noopener noreferrer" className="btn btn-ghost">
              Star on GitHub
            </a>
          </div>
          <p
            className="rise mt-7 mb-0 font-mono text-[0.7rem] tracking-[0.08em] text-faint"
            style={{ animationDelay: '320ms' }}
          >
            v1.2.2 · Windows · macOS · Linux · MIT
          </p>
        </div>

        <TestViewFigure />
      </section>

      {/* ───────────────────────── Features ───────────────────────── */}
      <section id="features" className="wrap pt-4 pb-24">
        <SectionRule label="crates/lgs · ui/" />
        <h2 className="display mt-12 mb-0 max-w-xl text-3xl font-bold sm:text-4xl">
          Everything the RGS contract needs, on your machine.
        </h2>
        <p className="mt-5 mb-0 max-w-xl leading-relaxed text-moss">
          The desktop app wraps a production-grade local game server around
          your math files and your front bundle. Point it at a folder; start
          spinning.
        </p>

        <div className="mt-12 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {FEATURES.map((feature) => (
            <article key={feature.title} className="card card-hover p-6">
              <h3 className="m-0 text-[1.02rem] font-semibold">{feature.title}</h3>
              <p className="mt-2.5 mb-0 text-sm leading-relaxed text-moss">{feature.desc}</p>
            </article>
          ))}
        </div>
      </section>

      {/* ───────────────────────── Cloud / V2 ───────────────────────── */}
      <section id="cloud" className="wrap pt-4 pb-24">
        <SectionRule label="crates/server · v2 branch" />
        <div className="mt-12 grid gap-12 lg:grid-cols-[1fr_1fr] lg:gap-16">
          <div>
            <h2 className="display mt-0 mb-0 text-3xl font-bold sm:text-4xl">
              One engine, three surfaces.
            </h2>
            <p className="mt-5 mb-0 leading-relaxed text-moss">
              V2 turns the workbench into a platform. The same Rust engine that
              powers your local loop moves into the cloud: a web workbench for
              your team, and real hosted game instances behind every share
              link.
            </p>
            <div className="mt-8 space-y-5">
              {SURFACES.map((surface, i) => (
                <div key={surface.title} className="flex gap-4">
                  <span className="mt-0.5 font-mono text-xs text-mint">{`0${i + 1}`}</span>
                  <div>
                    <h3 className="m-0 text-[0.98rem] font-semibold">{surface.title}</h3>
                    <p className="mt-1.5 mb-0 text-sm leading-relaxed text-moss">
                      {surface.desc}
                    </p>
                  </div>
                </div>
              ))}
            </div>
          </div>

          <div>
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
            <div className="mt-4 space-y-4">
              {CLOUD_POINTS.map((point) => (
                <div key={point.title} className="card p-5">
                  <h3 className="m-0 text-[0.92rem] font-semibold">{point.title}</h3>
                  <p className="mt-1.5 mb-0 text-sm leading-relaxed text-moss">{point.desc}</p>
                </div>
              ))}
            </div>
            <a
              href={`${REPO}/blob/v2/V2.md`}
              target="_blank"
              rel="noopener noreferrer"
              className="btn btn-ghost mt-6"
            >
              Read the V2 plan
            </a>
          </div>
        </div>
      </section>

      {/* ───────────────────────── Pricing ───────────────────────── */}
      <section id="pricing" className="wrap pt-4 pb-24">
        <SectionRule label="pricing" />
        <h2 className="display mt-12 mb-0 max-w-xl text-3xl font-bold sm:text-4xl">
          Self-hosting is free. Forever.
        </h2>
        <p className="mt-5 mb-0 max-w-2xl leading-relaxed text-moss">
          The whole platform is open source; the subscription sells hosting and
          nothing else: zero-install access, wildcard play subdomains, storage,
          backups and updates. There is no feature gating and no enterprise
          edition.
        </p>

        <div className="mt-12 grid gap-4 lg:grid-cols-3">
          {/* Self-host */}
          <article className="card flex flex-col p-7">
            <h3 className="m-0 text-base font-semibold">Self-host</h3>
            <p className="display mt-4 mb-0 text-4xl font-bold">
              €0
              <span className="font-sans ml-2 text-sm font-normal text-faint">forever</span>
            </p>
            <ul className="mt-6 mb-0 flex-1 space-y-2.5 pl-0 text-sm text-moss" style={{ listStyle: 'none' }}>
              <li>Every feature, no exceptions</li>
              <li>Single binary + Postgres + Caddy</li>
              <li>Your infra, your data</li>
              <li>Community support</li>
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

          {/* Solo */}
          <article className="card flex flex-col border-amber/35 p-7">
            <div className="flex items-center justify-between gap-3">
              <h3 className="m-0 text-base font-semibold">Solo</h3>
              <span className="rounded-full border border-amber/40 px-2.5 py-1 font-mono text-[0.62rem] tracking-[0.1em] text-amber uppercase">
                Cloud
              </span>
            </div>
            <p className="display mt-4 mb-0 text-4xl font-bold">
              €5
              <span className="font-sans ml-2 text-sm font-normal text-faint">/ month</span>
            </p>
            <ul className="mt-6 mb-0 flex-1 space-y-2.5 pl-0 text-sm text-moss" style={{ listStyle: 'none' }}>
              <li>1 user, unlimited games</li>
              <li>10 GB math storage</li>
              <li>Share links, fair-use sessions</li>
              <li>14-day trial, no card upfront</li>
            </ul>
            <span className="btn btn-ghost mt-7 w-full cursor-default opacity-60" aria-disabled="true">
              Coming with V2
            </span>
          </article>

          {/* Team */}
          <article className="card flex flex-col p-7">
            <div className="flex items-center justify-between gap-3">
              <h3 className="m-0 text-base font-semibold">Team</h3>
              <span className="rounded-full border border-line2 px-2.5 py-1 font-mono text-[0.62rem] tracking-[0.1em] text-moss uppercase">
                Cloud
              </span>
            </div>
            <p className="display mt-4 mb-0 text-4xl font-bold">
              €15
              <span className="font-sans ml-2 text-sm font-normal text-faint">/ month</span>
            </p>
            <ul className="mt-6 mb-0 flex-1 space-y-2.5 pl-0 text-sm text-moss" style={{ listStyle: 'none' }}>
              <li>Up to 10 members</li>
              <li>50 GB math storage</li>
              <li>Higher share-session quotas</li>
              <li>Custom play subdomain</li>
            </ul>
            <span className="btn btn-ghost mt-7 w-full cursor-default opacity-60" aria-disabled="true">
              Coming with V2
            </span>
          </article>
        </div>

        <p className="mt-6 mb-0 font-mono text-[0.7rem] tracking-[0.06em] text-faint">
          draft pricing · final numbers land with V2 · annual billing planned at two months free
        </p>
      </section>

      {/* ───────────────────────── Open source ───────────────────────── */}
      <section id="open-source" className="wrap pt-4 pb-24">
        <SectionRule label="LICENSE" />
        <div className="mt-12 grid items-start gap-12 lg:grid-cols-[1.1fr_1fr]">
          <div>
            <h2 className="display mt-0 mb-0 max-w-md text-3xl font-bold sm:text-4xl">
              Open source is the DNA.
            </h2>
            <p className="mt-5 mb-0 max-w-lg leading-relaxed text-moss">
              Every line of the platform ships in the open: desktop app,
              engine, and the V2 cloud server. If we ever disappear, your
              workflow doesn&apos;t.
            </p>
            <dl className="mt-8 space-y-5">
              <div className="flex gap-4">
                <dt className="w-24 shrink-0 font-mono text-xs text-mint">MIT</dt>
                <dd className="m-0 text-sm leading-relaxed text-moss">
                  Desktop app and the <code className="text-dim">lgs</code> engine. Use them
                  anywhere, for anything.
                </dd>
              </div>
              <div className="flex gap-4">
                <dt className="w-24 shrink-0 font-mono text-xs text-mint">AGPL-3.0</dt>
                <dd className="m-0 text-sm leading-relaxed text-moss">
                  The V2 cloud server. Self-hosting is untouched; the licence
                  only stops someone reselling our server as a closed hosted
                  service.
                </dd>
              </div>
            </dl>
          </div>

          <div className="card overflow-x-auto p-6">
            <p className="m-0 font-mono text-[0.78rem] leading-loose whitespace-pre text-dim">
              <span className="text-faint">$</span> git clone {REPO}
              {'\n'}
              <span className="text-faint">$</span> docker compose up
              {'\n'}
              <span className="text-faint"># the same platform we host — ships with V2</span>
            </p>
          </div>
        </div>
      </section>

      {/* ───────────────────────── FAQ ───────────────────────── */}
      <section id="faq" className="wrap pt-4 pb-24">
        <SectionRule label="FAQ" />
        <div className="mt-12 grid gap-12 lg:grid-cols-[1fr_1.4fr]">
          <h2 className="display mt-0 mb-0 text-3xl font-bold sm:text-4xl">
            Fair questions.
          </h2>
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

      {/* ───────────────────────── Final CTA ───────────────────────── */}
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
            Download the desktop app and point it at your game, or watch the
            cloud platform take shape in the open.
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
            <a
              href={`${REPO}/blob/v2/V2.md`}
              target="_blank"
              rel="noopener noreferrer"
              className="btn btn-ghost"
            >
              Follow the V2 build
            </a>
          </div>
        </div>
      </section>
    </main>
  )
}
