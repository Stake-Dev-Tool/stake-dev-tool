<script lang="ts">
  /**
   * MathConstraints — the decluttered bet-level constraints table.
   *
   * One clean line per constraint (label · value · two limit gauges) instead of
   * the old duplicated-column table. Failing rows sort to the top (stable within
   * the pass/fail groups) and each row carries an `id` so the verdict callout can
   * anchor straight to it. Helper copy lives behind a per-row ⓘ toggle rather
   * than under every label. Single-value metrics render one value next to the
   * label and a compact limit gauge per star; per-reference-bet metrics
   * (max_exposure, max_bet_cost) render a value inside each star cell, captioned
   * with the reference bet it was measured at.
   */
  import type { ConstraintRow } from '$lib/api';
  import { formatMetric } from '$lib/format';
  import Card from '$lib/components/Card.svelte';

  type Props = {
    constraints: ConstraintRow[];
    referenceMaxBet2: number | null;
    referenceMaxBet3: number | null;
  };
  let { constraints, referenceMaxBet2, referenceMaxBet3 }: Props = $props();

  // One "what it is" line per constraint key (nothing for unknown keys).
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

  const labelOf = (row: ConstraintRow) => row.label || row.key || '—';
  const isPerRef = (row: ConstraintRow) => row.value2 != null || row.value3 != null;

  // Failing rows (fail either star) sort to the top; Array.sort is stable, so the
  // analyzer's original order is preserved within each group.
  let rows = $derived(
    [...constraints].sort((a, b) => Number(a.pass2 && a.pass3) - Number(b.pass2 && b.pass3))
  );

  // Per-row ⓘ help toggle (title= tooltip is always there; click expands a line).
  let open = $state<Record<string, boolean>>({});
</script>

{#snippet gauge(starLabel: string, c: ColData, showValue: boolean, caption: string)}
  <div
    class="rounded-md border px-2.5 py-2 {c.pass
      ? 'border-accent/20 bg-accent/5'
      : 'border-danger/30 bg-danger/10'}"
  >
    <div class="flex items-baseline justify-between gap-2">
      <span class="text-[10px] font-medium uppercase tracking-wide text-faint">
        {starLabel}{#if caption}<span class="text-faint/80"> · {caption}</span>{/if}
      </span>
      <span class="text-xs {c.pass ? 'text-accent' : 'text-danger'}" aria-hidden="true">
        {c.pass ? '✓' : '✗'}
      </span>
    </div>
    {#if showValue}
      <div class="mt-0.5 flex items-baseline justify-between gap-1">
        <span class="font-mono-tab text-sm text-text">{formatMetric(c.value)}</span>
        <span class="font-mono-tab text-xs text-faint">/ {limitText(c)}</span>
      </div>
    {:else}
      <div class="mt-0.5 font-mono-tab text-xs text-faint">limit {limitText(c)}</div>
    {/if}
    <div class="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
      {#if c.hasBar}
        <div class="h-full rounded-full {c.barClass} transition-all" style="width: {c.width}%"></div>
      {/if}
    </div>
  </div>
{/snippet}

<Card class="overflow-hidden">
  {#if rows.length === 0}
    <p class="px-4 py-8 text-center text-sm text-muted">No constraints reported.</p>
  {:else}
    <!-- Column header (desktop only) — aligns with the row layout below. -->
    <div
      class="hidden items-center gap-4 border-b border-border px-4 py-2 text-xs uppercase tracking-wide text-faint sm:flex"
    >
      <div class="flex-1">Constraint</div>
      <div class="grid w-[22rem] flex-none grid-cols-2 gap-2">
        <div>2★ limit</div>
        <div>3★ limit</div>
      </div>
    </div>

    {#each rows as row (row.key)}
      {@const c2 = col(row, 2)}
      {@const c3 = col(row, 3)}
      {@const perRef = isPerRef(row)}
      {@const help = CONSTRAINT_HELP[row.key]}
      {@const failed = !row.pass2 || !row.pass3}
      <div
        id="c-{row.key}"
        class="scroll-mt-28 border-b border-border/60 px-4 py-2.5 last:border-0 {failed
          ? 'bg-danger/[0.03]'
          : ''}"
      >
        <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-4">
          <!-- Constraint zone: label · ⓘ · single value -->
          <div class="min-w-0 sm:flex-1">
            <div class="flex items-center gap-1.5">
              <span class="font-medium text-text">{labelOf(row)}</span>
              {#if help}
                <button
                  type="button"
                  title={help}
                  aria-label="What is {labelOf(row)}?"
                  aria-expanded={!!open[row.key]}
                  onclick={() => (open[row.key] = !open[row.key])}
                  class="inline-flex h-4 w-4 flex-none items-center justify-center rounded-full border border-border text-[10px] leading-none text-faint transition hover:border-border-strong hover:text-muted"
                >
                  i
                </button>
              {/if}
              {#if !perRef}
                <span class="ml-auto pl-2 font-mono-tab text-sm text-text">{formatMetric(row.value)}</span>
              {/if}
            </div>
            {#if help && open[row.key]}
              <p class="mt-1 max-w-md text-xs leading-relaxed text-muted">{help}</p>
            {/if}
          </div>

          <!-- Two limit gauges, side by side (stack under the label on mobile). -->
          <div class="grid grid-cols-2 gap-2 sm:w-[22rem] sm:flex-none">
            {@render gauge('2★', c2, perRef, perRef ? `at ${formatMetric(referenceMaxBet2)} bet` : '')}
            {@render gauge('3★', c3, perRef, perRef ? `at ${formatMetric(referenceMaxBet3)} bet` : '')}
          </div>
        </div>
      </div>
    {/each}
  {/if}
</Card>
