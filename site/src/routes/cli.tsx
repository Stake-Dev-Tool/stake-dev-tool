import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import SectionRule from '../components/SectionRule'

export const Route = createFileRoute('/cli')({
  head: () => ({
    meta: [
      { title: 'CLI & MCP — Stake Dev Tool' },
      {
        name: 'description',
        content:
          'Push math revisions from your terminal, your CI, or your AI agent. The sdt CLI and its built-in MCP server drive the whole platform.',
      },
    ],
  }),
  component: CliPage,
})

function CopyBlock({ label, text }: { label?: string; text: string }) {
  const [copied, setCopied] = useState(false)
  const copy = () => {
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    })
  }
  return (
    <div className="card relative mt-4 !p-0">
      {label ? (
        <div className="border-b border-line px-4 py-2 text-xs tracking-wide text-moss uppercase">
          {label}
        </div>
      ) : null}
      <button
        type="button"
        onClick={copy}
        className="absolute top-2 right-2 rounded border border-line px-2 py-1 text-xs text-moss transition-colors hover:text-ink"
      >
        {copied ? 'Copied' : 'Copy'}
      </button>
      <pre className="m-0 overflow-x-auto p-4 text-[0.82rem] leading-relaxed whitespace-pre-wrap">
        <code>{text}</code>
      </pre>
    </div>
  )
}

const INSTALL = `cargo install --git https://github.com/Stake-Dev-Tool/stake-dev-tool --branch v2 cli`

const QUICKSTART = `# sign in once (device flow — approve in the dashboard)
sdt login --save --server https://app.stakedevtool.com

# push a math folder as a new revision (only changed files upload)
sdt push ./math/my-game --workspace my-team --game my-game -m "tuned bonus RTP" --wait-stats

# browse
sdt revisions --workspace my-team --game my-game
sdt diff --workspace my-team --game my-game 42 41`

const CI_SNIPPET = `# .github/workflows/math.yml — push every merged math change
- name: Push math revision
  env:
    SDT_SERVER: https://app.stakedevtool.com
    SDT_TOKEN: \${{ secrets.SDT_TOKEN }}   # API token with the push:math scope
  run: sdt push ./math/my-game --workspace my-team --game my-game -m "\${{ github.event.head_commit.message }}" --json`

const MCP_CLAUDE = `claude mcp add sdt -e SDT_TOKEN=sdt_pat_... -- sdt mcp --server https://app.stakedevtool.com`

const MCP_GENERIC = `{
  "mcpServers": {
    "sdt": {
      "command": "sdt",
      "args": ["mcp", "--server", "https://app.stakedevtool.com"],
      "env": { "SDT_TOKEN": "sdt_pat_..." }
    }
  }
}`

const PROMPT_INSTALL = `Install the Stake Dev Tool CLI on this machine:
1. Check Rust is available (rustup), then run:
   cargo install --git https://github.com/Stake-Dev-Tool/stake-dev-tool --branch v2 cli
2. Run \`sdt login --save --server https://app.stakedevtool.com\` and give me the
   verification URL + code so I can approve the device in my dashboard.
3. Confirm everything works with \`sdt whoami\`.`

const PROMPT_MCP = `Set up the Stake Dev Tool MCP server for this client:
1. If the \`sdt\` binary is missing, install it:
   cargo install --git https://github.com/Stake-Dev-Tool/stake-dev-tool --branch v2 cli
2. Ask me for an API token (I create it at https://app.stakedevtool.com/account,
   scope push:math), then register the MCP server, e.g. for Claude Code:
   claude mcp add sdt -e SDT_TOKEN=<token> -- sdt mcp --server https://app.stakedevtool.com
3. Verify by calling the list_workspaces tool and showing me the result.`

const PROMPT_PUSH = `Push the math folder at <path> to my Stake Dev Tool workspace <workspace>,
game <game>, with a clear one-line message describing the change. Use
\`sdt push … --wait-stats\` and show me the per-mode RTP / max win table when
it lands. If the CLI is not installed or not logged in, set that up first.`

const TOOLS = [
  ['list_workspaces', 'workspaces you belong to'],
  ['list_games / list_revisions', 'catalogue per workspace and game'],
  ['get_revision', 'files + per-mode bet stats (RTP, max win, hit rate)'],
  ['diff_revisions', 'file changes and RTP deltas between two revisions'],
  ['push_math', 'push a local math folder as a new revision (dedup upload)'],
  ['pull_revision', 'download a revision to disk, hash-verified'],
] as const

function CliPage() {
  return (
    <main className="wrap pt-16 pb-8">
      <SectionRule label="crates/cli" />
      <h1 className="display mt-12 mb-0 max-w-2xl text-4xl font-bold sm:text-5xl">
        Your terminal, your CI, your agent.
      </h1>
      <p className="mt-6 mb-0 max-w-xl leading-relaxed text-moss">
        <code>sdt</code> drives the whole platform: push math revisions with
        deduplicated uploads, read stats and diffs, pull any revision back.
        And <code>sdt mcp</code> turns it into an MCP server, so AI agents can
        do all of it for you.
      </p>

      <section className="mt-16">
        <h2 className="display text-2xl font-bold">Install</h2>
        <p className="mt-3 max-w-xl text-moss">
          Grab a{' '}
          <a
            href="https://github.com/Stake-Dev-Tool/stake-dev-tool/releases?q=sdt-v&expanded=true"
            target="_blank"
            rel="noopener noreferrer"
          >
            prebuilt binary
          </a>{' '}
          (Linux x64, Windows x64, macOS arm64) and put it on your PATH — or
          build it with a Rust toolchain (1.90+):
        </p>
        <CopyBlock text={INSTALL} />
      </section>

      <section className="mt-14">
        <h2 className="display text-2xl font-bold">Quick start</h2>
        <CopyBlock text={QUICKSTART} />
        <p className="mt-3 max-w-xl text-sm text-moss">
          No CI? No terminal? You can also push a folder straight from the
          dashboard — drag it onto your game page.
        </p>
      </section>

      <section className="mt-14">
        <h2 className="display text-2xl font-bold">In CI</h2>
        <p className="mt-3 max-w-xl text-moss">
          Create an API token with the <code>push:math</code> scope in the
          dashboard, store it as a secret, and every math build ships itself.
        </p>
        <CopyBlock label="GitHub Actions" text={CI_SNIPPET} />
      </section>

      <section className="mt-14">
        <SectionRule label="sdt mcp" />
        <h2 className="display mt-10 text-2xl font-bold">The MCP server</h2>
        <p className="mt-3 max-w-xl leading-relaxed text-moss">
          <code>sdt mcp</code> speaks the Model Context Protocol over stdio.
          Register it once and your agent can push math, read RTP diffs and
          pull revisions — with your token, on your workspaces.
        </p>
        <CopyBlock label="Claude Code" text={MCP_CLAUDE} />
        <CopyBlock label="Any MCP client (stdio)" text={MCP_GENERIC} />
        <div className="card mt-6">
          <ul className="m-0 list-none space-y-2 p-0">
            {TOOLS.map(([name, desc]) => (
              <li key={name} className="flex flex-wrap gap-x-3 text-sm">
                <code className="text-ink">{name}</code>
                <span className="text-moss">{desc}</span>
              </li>
            ))}
          </ul>
        </div>
      </section>

      <section className="mt-14">
        <h2 className="display text-2xl font-bold">Prompts for your AI</h2>
        <p className="mt-3 max-w-xl text-moss">
          Minimal, copy-paste prompts. Give one to Claude Code (or any capable
          agent) and it sets everything up itself.
        </p>
        <CopyBlock label="Install + sign in" text={PROMPT_INSTALL} />
        <CopyBlock label="Set up the MCP server" text={PROMPT_MCP} />
        <CopyBlock label="Push math" text={PROMPT_PUSH} />
      </section>
    </main>
  )
}
