<script lang="ts">
  /**
   * MathModePanel — one mode workspace instead of three stacked sections. The
   * mode cards stay as the selector (the active card is visibly lit); everything
   * that follows is pinned into a SINGLE card whose header keeps the selected
   * mode name in view while internal Tabs swap between Metrics, Compliance and
   * Distribution. The parent owns which mode is selected (it resets on revision
   * navigation); this component owns only the active tab, so switching modes
   * keeps the tab and switching tabs keeps the mode.
   */
  import type { ModeAnalysis, Volatility } from '$lib/api';
  import { pct, formatOdds, formatSpins, formatMetric, formatCount, xmult } from '$lib/format';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';
  import Tabs from '$lib/components/Tabs.svelte';

  type Props = {
    modes: ModeAnalysis[];
    selectedModeName: string | null;
    onselect: (mode: string) => void;
  };
  let { modes, selectedModeName, onselect }: Props = $props();

  let selected = $derived<ModeAnalysis | null>(
    modes.find((m) => m.mode === selectedModeName) ?? modes[0] ?? null
  );

  type ModeTab = 'metrics' | 'compliance' | 'distribution';
  let activeTab = $state<ModeTab>('metrics');

  /** Badge tone for a volatility label — low = sky (info), medium = amber, high = red. */
  function volTone(v: Volatility | null): 'info' | 'warn' | 'danger' | 'neutral' {
    if (v === 'low') return 'info';
    if (v === 'medium') return 'warn';
    if (v === 'high') return 'danger';
    return 'neutral';
  }

  /** A mode is compliant when it has checks and every one passes. */
  function modeVerdict(m: ModeAnalysis): { has: boolean; ok: boolean } {
    const has = m.compliance.length > 0;
    return { has, ok: has && m.compliance.every((c) => c.pass) };
  }

  /** Subtle emerald row tint proportional to a bucket's RTP contribution weight. */
  function tintStyle(contribution: number | null, max: number): string {
    if (contribution == null || max <= 0) return '';
    const a = Math.max(0, Math.min(0.16, (contribution / max) * 0.16));
    return `background-color: rgba(16, 185, 129, ${a.toFixed(3)})`;
  }

  let verdict = $derived(selected ? modeVerdict(selected) : { has: false, ok: false });
  let failCount = $derived(selected ? selected.compliance.filter((c) => !c.pass).length : 0);
  let maxContribution = $derived(
    selected ? selected.distribution.reduce((mx, b) => Math.max(mx, b.rtp_contribution ?? 0), 0) : 0
  );
</script>

{#snippet tile(label: string, value: string)}
  <div class="rounded-md border border-border bg-surface-2/40 px-3 py-2.5">
    <div class="text-xs text-faint">{label}</div>
    <div class="mt-0.5 font-mono-tab text-sm text-text">{value}</div>
  </div>
{/snippet}

{#snippet streakTile(label: string, value: string, note: string)}
  <div class="rounded-md border border-border bg-surface-2/40 p-4">
    <div class="text-xs text-faint">{label}</div>
    <div class="mt-1 font-mono-tab text-xl font-semibold text-text">{value}</div>
    <div class="mt-1.5 text-xs leading-relaxed text-muted">{note}</div>
  </div>
{/snippet}

<section id="modes" class="scroll-mt-28">
  <SectionHeader title="Game modes">
    {#snippet children()}
      Select a mode to inspect its metrics, compliance, and hit-rate distribution.
    {/snippet}
  </SectionHeader>

  <!-- Selector: the mode cards grid (the active card is lit). -->
  <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
    {#each modes as m (m.mode)}
      {@const v = modeVerdict(m)}
      <button
        type="button"
        onclick={() => onselect(m.mode)}
        aria-pressed={selectedModeName === m.mode}
        class="flex flex-col gap-3 rounded-lg border bg-surface p-4 text-left transition {selectedModeName ===
        m.mode
          ? 'border-accent/60 ring-1 ring-accent/30'
          : 'border-border hover:border-border-strong'}"
      >
        <div class="flex items-start justify-between gap-2">
          <span class="min-w-0 truncate font-semibold text-text">{m.mode || '—'}</span>
          <div class="flex flex-shrink-0 items-center gap-1.5">
            <Badge>{m.cost == null ? '—' : `${formatMetric(m.cost)}x`}</Badge>
            <Badge tone={volTone(m.volatility)}>{m.volatility ?? '—'}</Badge>
          </div>
        </div>
        <div>
          {#if v.has}
            <Badge tone={v.ok ? 'accent' : 'danger'}>{v.ok ? 'Compliant' : 'Issues'}</Badge>
          {:else}
            <Badge>no checks</Badge>
          {/if}
        </div>
        <div class="grid grid-cols-4 gap-2 border-t border-border/60 pt-3 text-center">
          <div>
            <div class="text-[10px] uppercase tracking-wide text-faint">RTP</div>
            <div class="mt-0.5 font-mono-tab text-sm text-text">{pct(m.rtp)}</div>
          </div>
          <div>
            <div class="text-[10px] uppercase tracking-wide text-faint">Hit</div>
            <div class="mt-0.5 font-mono-tab text-sm text-text">{pct(m.hit_rate)}</div>
          </div>
          <div>
            <div class="text-[10px] uppercase tracking-wide text-faint">Max</div>
            <div class="mt-0.5 font-mono-tab text-sm text-text">{xmult(m.max_win)}</div>
          </div>
          <div>
            <div class="text-[10px] uppercase tracking-wide text-faint">B/E</div>
            <div class="mt-0.5 font-mono-tab text-sm text-text">{pct(m.break_even_miss_prob)}</div>
          </div>
        </div>
      </button>
    {/each}
  </div>

  {#if selected}
    <!-- One workspace card: mode name stays in the header while tabs swap. -->
    <Card class="mt-4 overflow-hidden">
      <div
        class="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 border-b border-border px-5 py-4"
      >
        <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span class="font-semibold text-text">{selected.mode || '—'}</span>
          <span class="text-faint">— detailed analysis</span>
          <Badge>{selected.cost == null ? '—' : `${formatMetric(selected.cost)}x`}</Badge>
          <Badge tone={volTone(selected.volatility)}>{selected.volatility ?? '—'} volatility</Badge>
        </div>
        {#if verdict.has}
          <Badge tone={verdict.ok ? 'accent' : 'danger'}>
            {verdict.ok ? 'Compliant' : `${failCount} issue${failCount === 1 ? '' : 's'}`}
          </Badge>
        {:else}
          <Badge>no checks</Badge>
        {/if}
      </div>

      <Tabs
        class="px-5"
        tabs={[
          { id: 'metrics', label: 'Metrics' },
          { id: 'compliance', label: 'Compliance', badge: selected.compliance.length || undefined },
          {
            id: 'distribution',
            label: 'Distribution',
            badge: selected.distribution.length || undefined
          }
        ]}
        active={activeTab}
        onselect={(id) => (activeTab = id as ModeTab)}
      />

      <div class="p-5">
        {#if activeTab === 'metrics'}
          <div class="flex flex-col gap-6">
            <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
              {@render tile('Std dev', formatMetric(selected.std_dev))}
              {@render tile('Entries', formatCount(selected.entries))}
              {@render tile('Zero rate', pct(selected.zero_prob))}
              {@render tile('Mean', xmult(selected.rtp))}
              {@render tile('Hit rate', pct(selected.hit_rate))}
              {@render tile('Min win', xmult(selected.min_win))}
              {@render tile('Max win', xmult(selected.max_win))}
              {@render tile('Max-win odds', formatOdds(selected.max_win_odds))}
              {@render tile('Unique payouts', formatCount(selected.unique_payouts))}
              {@render tile('Sub-bet rate', pct(selected.sub_bet_prob))}
            </div>

            <!-- Outcome breakdown -->
            <div>
              <div class="mb-2 text-xs font-semibold uppercase tracking-wide text-faint">
                Outcome breakdown
              </div>
              <div class="flex h-3 w-full overflow-hidden rounded-full bg-surface-2">
                <div
                  class="h-full bg-border-strong"
                  style="width: {(selected.zero_prob ?? 0) * 100}%"
                  title="Dead"
                ></div>
                <div
                  class="h-full bg-warn"
                  style="width: {(selected.sub_bet_prob ?? 0) * 100}%"
                  title="Sub-bet"
                ></div>
                <div
                  class="h-full bg-accent"
                  style="width: {(selected.win_prob ?? 0) * 100}%"
                  title="Win"
                ></div>
              </div>
              <div class="mt-2 flex flex-wrap gap-x-5 gap-y-1 text-xs">
                <span class="inline-flex items-center gap-1.5">
                  <span class="h-2 w-2 rounded-full bg-border-strong"></span>
                  <span class="text-muted">Dead</span>
                  <span class="font-mono-tab text-text">{pct(selected.zero_prob)}</span>
                </span>
                <span class="inline-flex items-center gap-1.5">
                  <span class="h-2 w-2 rounded-full bg-warn"></span>
                  <span class="text-muted">Sub-bet</span>
                  <span class="font-mono-tab text-text">{pct(selected.sub_bet_prob)}</span>
                </span>
                <span class="inline-flex items-center gap-1.5">
                  <span class="h-2 w-2 rounded-full bg-accent"></span>
                  <span class="text-muted">Win</span>
                  <span class="font-mono-tab text-text">{pct(selected.win_prob)}</span>
                </span>
              </div>
            </div>

            <!-- Streaks -->
            <div>
              <div class="mb-2 text-xs font-semibold uppercase tracking-wide text-faint">Streaks</div>
              <div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
                {@render streakTile(
                  'Avg spins between wins',
                  formatSpins(selected.avg_spins_any_win),
                  'Typical number of spins between any paying spin.'
                )}
                {@render streakTile(
                  'Worst dry streak',
                  formatSpins(selected.worst_zero_streak),
                  'Longest run of dead spins a 1-in-1,000 unlucky session hits.'
                )}
                {@render streakTile(
                  'Avg spins between profit',
                  formatSpins(selected.avg_spins_profit),
                  'Typical spins between spins that pay more than the stake.'
                )}
                {@render streakTile(
                  'Worst losing streak',
                  formatSpins(selected.worst_loss_streak),
                  'Longest run without a profitable spin at 1-in-1,000 bad luck.'
                )}
              </div>
            </div>
          </div>
        {:else if activeTab === 'compliance'}
          {#if selected.compliance.length === 0}
            <p class="py-4 text-center text-sm text-muted">
              No compliance checks reported for this mode.
            </p>
          {:else}
            <ul class="flex flex-col gap-3">
              {#each selected.compliance as check (check.check)}
                <li class="flex items-start gap-3">
                  <span
                    class="mt-0.5 flex h-5 w-5 flex-shrink-0 items-center justify-center rounded-full text-xs {check.pass
                      ? 'bg-accent/15 text-accent'
                      : 'bg-danger/15 text-danger'}"
                    aria-hidden="true"
                  >
                    {check.pass ? '✓' : '✗'}
                  </span>
                  <div class="min-w-0">
                    <div class="text-sm font-medium text-text">
                      {check.label || check.check || '—'}
                    </div>
                    <div class="mt-0.5 text-xs text-muted">
                      Expected <span class="font-mono-tab text-text">{check.expected || '—'}</span>
                      <span aria-hidden="true" class="text-faint">→</span>
                      Result
                      <span class="font-mono-tab {check.pass ? 'text-text' : 'text-danger'}"
                        >{check.result || '—'}</span
                      >
                    </div>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        {:else if selected.distribution.length === 0}
          <p class="py-4 text-center text-sm text-muted">No distribution reported for this mode.</p>
        {:else}
          <p class="mb-3 text-xs text-faint">
            Row shading = share of RTP — darker rows contribute more of the return.
          </p>
          <div class="-mx-5 overflow-x-auto">
            <table class="w-full min-w-[40rem] text-sm">
              <thead>
                <tr
                  class="border-b border-border text-left text-xs uppercase tracking-wide text-faint"
                >
                  <th class="px-5 py-2.5 font-medium">Range</th>
                  <th class="px-5 py-2.5 font-medium text-right">Count</th>
                  <th class="px-5 py-2.5 font-medium text-right">Effective hit-rate</th>
                  <th class="px-5 py-2.5 font-medium text-right">RTP contribution</th>
                </tr>
              </thead>
              <tbody>
                {#each selected.distribution as b, i (i)}
                  <tr
                    class="border-b border-border/60 last:border-0"
                    style={tintStyle(b.rtp_contribution, maxContribution)}
                  >
                    <td class="px-5 py-2.5 font-mono-tab text-muted">
                      ( {formatMetric(b.from)}, {b.to == null ? '∞' : formatMetric(b.to)} )
                    </td>
                    <td class="px-5 py-2.5 text-right font-mono-tab text-muted">
                      {formatCount(b.count)}
                    </td>
                    <td class="px-5 py-2.5 text-right font-mono-tab text-muted">
                      {formatMetric(b.effective_hit_rate, 2)}
                    </td>
                    <td class="px-5 py-2.5 text-right font-mono-tab text-text">
                      {pct(b.rtp_contribution)}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>
    </Card>
  {/if}
</section>
