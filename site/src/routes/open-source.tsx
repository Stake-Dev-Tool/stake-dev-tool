import { createFileRoute } from '@tanstack/react-router'
import SectionRule from '../components/SectionRule'

export const Route = createFileRoute('/open-source')({
  head: () => ({
    meta: [
      { title: 'Open source — Stake Dev Tool' },
      {
        name: 'description',
        content:
          'The entire platform ships in the open: desktop app and engine under MIT, cloud server under AGPL-3.0. Self-host everything with a single binary, Postgres and Caddy.',
      },
    ],
  }),
  component: OpenSourcePage,
})

const REPO = 'https://github.com/Stake-Dev-Tool/stake-dev-tool'

function OpenSourcePage() {
  return (
    <main className="wrap pt-16 pb-8">
      <SectionRule label="LICENSE" />
      <div className="mt-12 grid items-start gap-12 lg:grid-cols-[1.1fr_1fr]">
        <div>
          <h1 className="display mt-0 mb-0 max-w-md text-4xl font-bold sm:text-5xl">
            Open source is the DNA.
          </h1>
          <p className="mt-6 mb-0 max-w-lg leading-relaxed text-moss">
            Every line of the platform ships in the open: desktop app, engine,
            and the cloud server. If we ever disappear, your workflow
            doesn&apos;t.
          </p>
          <dl className="mt-10 space-y-6">
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
                The cloud server. Self-hosting is untouched; the licence only
                stops someone reselling our server as a closed hosted service.
              </dd>
            </div>
          </dl>

          <div className="mt-10 flex flex-wrap gap-3">
            <a
              href={REPO}
              target="_blank"
              rel="noopener noreferrer"
              className="btn btn-primary"
            >
              Star on GitHub
            </a>
            <a
              href={`${REPO}/blob/main/CONTRIBUTING.md`}
              target="_blank"
              rel="noopener noreferrer"
              className="btn btn-ghost"
            >
              Contribute
            </a>
          </div>
        </div>

        <div className="card overflow-x-auto p-6">
          <p className="m-0 font-mono text-[0.78rem] leading-loose whitespace-pre text-dim">
            <span className="text-faint">$</span> git clone {REPO}
            {'\n'}
            <span className="text-faint">$</span> docker compose up
            {'\n'}
            <span className="text-faint"># the same platform we host</span>
          </p>
        </div>
      </div>
    </main>
  )
}
