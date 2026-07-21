<script lang="ts">
  import { page } from '$app/state';
  import {
    api,
    ApiError,
    type RevisionDetail,
    type RevisionAnalysis,
    type ConstraintRow,
    type ModeAnalysis,
    type Volatility
  } from '$lib/api';
  import {
    errorText,
    relativeAge,
    pct,
    formatOdds,
    formatSpins,
    formatMetric,
    formatCount
  } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';

  let slug = $derived(page.params.slug ?? '');
  let game = $derived(page.params.game ?? '');
  let numParam = $derived(page.params.number ?? '');
  let revNum = $derived(Number(numParam));

  let detail = $state<RevisionDetail | null>(null);
  let loading = $state(true);
  let loadError = $state('');
  let notFound = $state(false);

  let stats = $derived(detail?.stats ?? null);
  let statsStatus = $derived(stats?.status ?? null);
  let analysis = $derived<RevisionAnalysis | null>(stats?.analysis ?? null);
  let constraints = $derived<ConstraintRow[]>(analysis?.constraints ?? []);
  let modes = $derived<ModeAnalysis[]>(analysis?.modes ?? []);

  // Selected mode for the detail / checklist / distribution sections. Defaults to
  // the cost-1 base mode (falls back to the first) once the analysis arrives, and
  // resets if the user navigates to a revision whose modes don't include it.
  let selectedModeName = $state<string | null>(null);
  $effect(() => {
    const ms = modes;
    if (ms.length === 0) {
      if (selectedModeName !== null) selectedModeName = null;
      return;
    }
    if (!selectedModeName || !ms.some((m) => m.mode === selectedModeName)) {
      selectedModeName = (ms.find((m) => m.cost === 1) ?? ms[0]).mode;
    }
  });
  let selected = $derived<ModeAnalysis | null>(
    modes.find((m) => m.mode === selectedModeName) ?? modes[0] ?? null
  );

  // Reload whenever the route params change (the component is reused across
  // /r/:number navigations).
  $effect(() => {
    void slug;
    void game;
    void numParam;
    void load(true);
  });

  // Poll while stats are pending so the report appears the moment the analyzer
  // finishes; the effect teardown stops the poll on ok/error and on unmount.
  $effect(() => {
    if (statsStatus !== 'pending') return;
    const id = setInterval(() => void load(false), 3000);
    return () => clearInterval(id);
  });

  async function load(initial: boolean) {
    if (initial) {
      loading = true;
      loadError = '';
      notFound = false;
    }
    try {
      detail = await api.games.revision(slug, game, numParam);
    } catch (e) {
      // Only surface errors on the initial load — a failed background poll keeps
      // the current view and retries on the next tick.
      if (initial) {
        if (e instanceof ApiError && e.status === 404) notFound = true;
        else loadError = errorText(e);
      }
    } finally {
      if (initial) loading = false;
    }
  }

  // --- Presentation helpers --------------------------------------------------

  /** Bet multiplier with an "x" suffix ("6,750x", "0.96x"); em-dash when absent. */
  function xmult(n: number | null | undefined): string {
    if (n == null || !Number.isFinite(n)) return '—';
    return `${formatMetric(n)}x`;
  }

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

  // One "what it is" line per constraint key (falls back to nothing for unknowns).
  const CONSTRAINT_HELP: Record<string, string> = {
    max_exposure: 'Peak payout the game can owe on a single reference bet.',
    max_payout_multiplier: 'Largest win multiple any single bet can pay.',
    max_bet_cost: 'Total stake charged for one reference bet.',
    cost_multiplier: 'Bonus-buy price as a multiple of the base bet.',
    base_volatility: 'Payout spread around the mean — must land inside the allowed band.',
    tail_prob_5000: 'Probability of a single payout of 5,000x or more.',
    tail_prob_10000: 'Probability of a single payout of 10,000x or more.',
    cvar: 'Average loss across the worst-case tail (conditional value at risk).',
    etl_40: 'Expected tail loss measured over a 40-bet horizon.',
    etl_10000: 'Expected tail loss measured over a 10,000-bet horizon.',
    etl_sum: 'Combined expected tail loss across the measured horizons.'
  };

  type ColData = {
    value: number | null;
    limit: number | null;
    low: number | null;
    pass: boolean;
    isRange: boolean;
    width: number;
    hasBar: boolean;
    barClass: string;
  };

  /**
   * Resolve one star column of a constraint row. Per-reference-bet metrics carry
   * value2/value3; single-value metrics carry `value` (used for both columns).
   * The bar tracks value / limit — amber ≥ 80%, red over the cap or on a fail;
   * range metrics colour purely by pass (the ratio is not meaningful two-sided).
   */
  function col(row: ConstraintRow, star: 2 | 3): ColData {
    const value = star === 2 ? (row.value2 ?? row.value) : (row.value3 ?? row.value);
    const limit = star === 2 ? row.limit2 : row.limit3;
    const low = star === 2 ? row.limit2_low : row.limit3_low;
    const pass = star === 2 ? row.pass2 : row.pass3;
    const isRange = low != null;
    let width = 0;
    let over = false;
    const hasBar = value != null && limit != null && limit > 0;
    if (hasBar) {
      const r = value! / limit!;
      over = r > 1;
      width = Math.max(0, Math.min(100, r * 100));
    }
    const barClass = isRange
      ? pass
        ? 'bg-accent'
        : 'bg-danger'
      : !pass || over
        ? 'bg-danger'
        : width >= 80
          ? 'bg-warn'
          : 'bg-accent';
    return { value, limit, low, pass, isRange, width, hasBar, barClass };
  }

  function limitText(c: ColData): string {
    if (c.isRange) return `${formatMetric(c.low)} – ${formatMetric(c.limit)}`;
    return formatMetric(c.limit);
  }

  /** Subtle emerald row tint proportional to a bucket's RTP contribution weight. */
  function tintStyle(contribution: number | null, max: number): string {
    if (contribution == null || max <= 0) return '';
    const a = Math.max(0, Math.min(0.16, (contribution / max) * 0.16));
    return `background-color: rgba(16, 185, 129, ${a.toFixed(3)})`;
  }
</script>

<svelte:head><title>Math report · rev {numParam} · {game} · Stake Cloud</title></svelte:head>

{#snippet constraintCol(c: ColData)}
  <td class="px-4 py-3 align-top {c.pass ? 'bg-accent/5' : 'bg-danger/10'}">
    <div class="flex items-baseline justify-between gap-2 text-sm">
      <span class="font-mono-tab text-text">{formatMetric(c.value)}</span>
      <span class="font-mono-tab text-faint">/ {limitText(c)}</span>
    </div>
    <div class="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
      {#if c.hasBar}
        <div class="h-full rounded-full {c.barClass} transition-all" style="width: {c.width}%"></div>
      {/if}
    </div>
    <div class="mt-1 flex items-center gap-1 text-xs {c.pass ? 'text-accent' : 'text-danger'}">
      <span aria-hidden="true">{c.pass ? '✓' : '✗'}</span>
      <span>{c.pass ? 'Pass' : 'Fail'}</span>
    </div>
  </td>
{/snippet}

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

<main class="mx-auto w-full max-w-5xl px-6 py-10">
  <a
    href={`/w/${slug}/g/${game}/r/${revNum}`}
    class="mb-6 inline-flex items-center gap-1.5 text-sm text-muted transition hover:text-text"
  >
    <span aria-hidden="true">←</span> rev {numParam}
  </a>

  {#if loading}
    <div class="flex items-center gap-3 py-16 text-muted"><span class="spinner"></span> Loading…</div>
  {:else if notFound}
    <Card class="flex flex-col items-center gap-3 border-dashed px-6 py-16 text-center">
      <span class="flex h-11 w-11 items-center justify-center rounded-full bg-surface-2 text-xl text-muted">?</span>
      <h1 class="text-lg font-semibold">Revision not found</h1>
      <p class="max-w-sm text-sm text-muted">This revision doesn't exist, or you don't have access to it.</p>
      <Button href={`/w/${slug}/g/${game}`} variant="outline" class="mt-2">Back to revisions</Button>
    </Card>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={() => load(true)}>Retry</Button>
    </Card>
  {:else if detail}
    <!-- 1 · Header -->
    <header class="mb-8">
      <div class="flex flex-wrap items-baseline gap-x-2.5 gap-y-1">
        <h1 class="text-2xl font-semibold tracking-tight">Math report</h1>
        <span class="text-sm text-muted">{game}</span>
        <span aria-hidden="true" class="text-faint">·</span>
        <span class="font-mono-tab text-sm text-text">rev {detail.number}</span>
        <span aria-hidden="true" class="text-faint">·</span>
        <span class="text-sm text-muted" title={detail.created_at}>{relativeAge(detail.created_at)}</span>
      </div>

      {#if analysis}
        <div class="mt-4 flex flex-wrap items-center gap-2">
          <Badge tone={analysis.two_star_compliant ? 'accent' : 'danger'}>
            2★ {analysis.two_star_compliant ? 'Compliant' : 'Non-compliant'}
          </Badge>
          <Badge tone={analysis.three_star_compliant ? 'accent' : 'danger'}>
            3★ {analysis.three_star_compliant ? 'Compliant' : 'Non-compliant'}
          </Badge>
          <Badge tone={analysis.stars === 0 ? 'neutral' : 'accent'}>
            {analysis.stars === 0 ? 'Not rated' : `${analysis.stars}★ awarded`}
          </Badge>
        </div>
        <p class="mt-3 text-sm text-muted">
          Cross-mode RTP consistency:
          <span class={analysis.cross_mode_rtp_pass ? 'font-medium text-accent' : 'font-medium text-danger'}>
            {analysis.cross_mode_rtp_pass ? 'Consistent' : 'Inconsistent'}
          </span>
          <span class="text-faint">
            · variance <span class="font-mono-tab">{formatMetric(analysis.cross_mode_rtp_variance, 4)}</span>
          </span>
        </p>
      {/if}
    </header>

    {#if analysis}
      <!-- 2 · Overall bet-level compliance -->
      <section class="mb-10">
        <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">
          Overall bet level compliance
        </h2>
        <Card class="overflow-hidden">
          {#if constraints.length === 0}
            <p class="px-4 py-8 text-center text-sm text-muted">No constraints reported.</p>
          {:else}
            <div class="overflow-x-auto">
              <table class="w-full min-w-[44rem] text-sm">
                <thead>
                  <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                    <th class="px-4 py-3 font-medium">Constraint</th>
                    <th class="px-4 py-3 font-medium">2 Star</th>
                    <th class="px-4 py-3 font-medium">3 Star</th>
                  </tr>
                </thead>
                <tbody>
                  {#each constraints as row (row.key)}
                    {@const c2 = col(row, 2)}
                    {@const c3 = col(row, 3)}
                    <tr class="border-b border-border/60 last:border-0">
                      <td class="px-4 py-3 align-top">
                        <div class="font-medium text-text">{row.label || row.key || '—'}</div>
                        {#if CONSTRAINT_HELP[row.key]}
                          <div class="mt-0.5 max-w-xs text-xs leading-relaxed text-muted">
                            {CONSTRAINT_HELP[row.key]}
                          </div>
                        {/if}
                      </td>
                      {@render constraintCol(c2)}
                      {@render constraintCol(c3)}
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        </Card>
        <p class="mt-2 text-xs text-muted">
          Limits evaluated at the reference max bets — 2★
          <span class="font-mono-tab text-text">{formatMetric(analysis.reference_max_bet_2)}</span>
          · 3★
          <span class="font-mono-tab text-text">{formatMetric(analysis.reference_max_bet_3)}</span>.
        </p>
      </section>

      <!-- 3 · Game modes grid -->
      {#if modes.length > 0}
        <section class="mb-10">
          <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Game modes</h2>
          <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {#each modes as m (m.mode)}
              {@const v = modeVerdict(m)}
              <button
                type="button"
                onclick={() => (selectedModeName = m.mode)}
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
        </section>
      {/if}

      {#if selected}
        <!-- 4 · Detailed metrics for the selected mode -->
        <section class="mb-10">
          <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
            <h2 class="text-sm font-semibold uppercase tracking-wide text-faint">Detailed metrics</h2>
            {#if modes.length > 1}
              <div class="flex flex-wrap gap-1.5">
                {#each modes as m (m.mode)}
                  <button
                    type="button"
                    onclick={() => (selectedModeName = m.mode)}
                    class="h-7 rounded-md border px-2.5 text-xs transition {selectedModeName === m.mode
                      ? 'border-accent/60 bg-accent/10 text-text'
                      : 'border-border bg-surface-2 text-muted hover:text-text'}"
                  >
                    {m.mode}
                  </button>
                {/each}
              </div>
            {/if}
          </div>

          <Card class="flex flex-col gap-6 p-5">
            <div>
              <div class="mb-2 flex items-center gap-2">
                <span class="font-semibold text-text">{selected.mode}</span>
                <Badge tone={volTone(selected.volatility)}>{selected.volatility ?? '—'} volatility</Badge>
              </div>
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
            </div>

            <!-- Outcome breakdown -->
            <div>
              <div class="mb-2 text-xs font-semibold uppercase tracking-wide text-faint">Outcome breakdown</div>
              <div class="flex h-3 w-full overflow-hidden rounded-full bg-surface-2">
                <div class="h-full bg-border-strong" style="width: {(selected.zero_prob ?? 0) * 100}%" title="Dead"></div>
                <div class="h-full bg-warn" style="width: {(selected.sub_bet_prob ?? 0) * 100}%" title="Sub-bet"></div>
                <div class="h-full bg-accent" style="width: {(selected.win_prob ?? 0) * 100}%" title="Win"></div>
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
          </Card>
        </section>

        <!-- 5 · Per-mode compliance checklist -->
        <section class="mb-10">
          <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">
            Compliance checklist · {selected.mode}
          </h2>
          <Card class="p-5">
            {#if selected.compliance.length === 0}
              <p class="py-4 text-center text-sm text-muted">No compliance checks reported for this mode.</p>
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
                      <div class="text-sm font-medium text-text">{check.label || check.check || '—'}</div>
                      <div class="mt-0.5 text-xs text-muted">
                        Expected <span class="font-mono-tab text-text">{check.expected || '—'}</span>
                        <span aria-hidden="true" class="text-faint">→</span>
                        Result <span class="font-mono-tab {check.pass ? 'text-text' : 'text-danger'}">{check.result || '—'}</span>
                      </div>
                    </div>
                  </li>
                {/each}
              </ul>
            {/if}
          </Card>
        </section>

        <!-- 6 · Hit-rate distribution -->
        <section>
          <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">
            Hit-rate distribution · {selected.mode}
          </h2>
          <Card class="overflow-hidden">
            {#if selected.distribution.length === 0}
              <p class="px-4 py-8 text-center text-sm text-muted">No distribution reported for this mode.</p>
            {:else}
              {@const maxC = selected.distribution.reduce((mx, b) => Math.max(mx, b.rtp_contribution ?? 0), 0)}
              <div class="overflow-x-auto">
                <table class="w-full min-w-[40rem] text-sm">
                  <thead>
                    <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                      <th class="px-4 py-3 font-medium">Range</th>
                      <th class="px-4 py-3 font-medium text-right">Count</th>
                      <th class="px-4 py-3 font-medium text-right">Effective hit-rate</th>
                      <th class="px-4 py-3 font-medium text-right">RTP contribution</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each selected.distribution as b, i (i)}
                      <tr class="border-b border-border/60 last:border-0" style={tintStyle(b.rtp_contribution, maxC)}>
                        <td class="px-4 py-3 font-mono-tab text-muted">
                          ( {formatMetric(b.from)}, {b.to == null ? '∞' : formatMetric(b.to)} )
                        </td>
                        <td class="px-4 py-3 text-right font-mono-tab text-muted">{formatCount(b.count)}</td>
                        <td class="px-4 py-3 text-right font-mono-tab text-muted">
                          {formatMetric(b.effective_hit_rate, 2)}
                        </td>
                        <td class="px-4 py-3 text-right font-mono-tab text-text">{pct(b.rtp_contribution)}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          </Card>
        </section>
      {/if}
    {:else if statsStatus === 'pending'}
      <Card class="flex items-center gap-3 px-4 py-10 text-muted">
        <span class="spinner"></span> Computing the compliance report… this refreshes automatically.
      </Card>
    {:else if statsStatus === 'error'}
      <Card class="px-4 py-6">
        <div class="mb-2"><Badge tone="danger">stats error</Badge></div>
        <p class="text-sm text-danger">
          {stats?.error || 'The server could not compute stats for this revision.'}
        </p>
      </Card>
    {:else}
      <!-- analysis absent: older revision predating the analyzer -->
      <Card class="flex flex-col items-center gap-3 border-dashed px-6 py-16 text-center">
        <span class="flex h-11 w-11 items-center justify-center rounded-full bg-surface-2 text-xl text-muted">
          ✦
        </span>
        <h2 class="text-lg font-semibold">No compliance analysis yet</h2>
        <p class="max-w-md text-sm leading-relaxed text-muted">
          This revision predates the compliance analyzer, so it has no Math report. Push a new
          revision to recompute it — the report appears here automatically once the analysis finishes.
        </p>
        <Button href={`/w/${slug}/g/${game}`} variant="outline" class="mt-2">Back to game</Button>
      </Card>
    {/if}
  {/if}
</main>
