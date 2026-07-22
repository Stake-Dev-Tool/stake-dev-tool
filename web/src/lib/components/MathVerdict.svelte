<script lang="ts">
  /**
   * MathVerdict — verdict first, problems first. The two honest star badges (the
   * rating is an estimate; Stake Engine decides), a one-line summary per star,
   * and — only when something fails — a compact red callout that lists exactly
   * the failing constraints with their value → limit, each anchoring to its row
   * in the table below (#c-<key>). Closes with the cross-mode RTP consistency
   * line so the whole verdict reads in one glance.
   */
  import type { RevisionAnalysis, ConstraintRow } from '$lib/api';
  import { pct, formatMetric } from '$lib/format';
  import Badge from '$lib/components/Badge.svelte';

  type Props = { analysis: RevisionAnalysis; constraints: ConstraintRow[] };
  let { analysis, constraints }: Props = $props();

  const labelOf = (row: ConstraintRow) => row.label || row.key || '—';
  const limitStr = (limit: number | null, low: number | null) =>
    low != null ? `${formatMetric(low)} – ${formatMetric(limit)}` : formatMetric(limit);

  let total = $derived(constraints.length);
  let fails2 = $derived(constraints.filter((c) => !c.pass2));
  let fails3 = $derived(constraints.filter((c) => !c.pass3));

  // Flat list of failing (constraint, star) pairs for the callout, in table order.
  type FailItem = { key: string; label: string; star: 2 | 3; value: number | null; limit: string };
  let failItems = $derived<FailItem[]>(
    constraints.flatMap((c) => {
      const items: FailItem[] = [];
      if (!c.pass2)
        items.push({
          key: c.key,
          label: labelOf(c),
          star: 2,
          value: c.value2 ?? c.value,
          limit: limitStr(c.limit2, c.limit2_low)
        });
      if (!c.pass3)
        items.push({
          key: c.key,
          label: labelOf(c),
          star: 3,
          value: c.value3 ?? c.value,
          limit: limitStr(c.limit3, c.limit3_low)
        });
      return items;
    })
  );
</script>

{#snippet summary(star: 2 | 3, fails: ConstraintRow[])}
  <div class="flex items-start gap-2 text-sm">
    <span class="mt-px flex-none {fails.length ? 'text-danger' : 'text-accent'}" aria-hidden="true">
      {fails.length ? '✗' : '✓'}
    </span>
    <span class="text-muted">
      {#if fails.length === 0}
        All <span class="font-mono-tab text-text">{total}</span> constraints within
        <span class="text-text">{star}★</span> limits
      {:else}
        <span class="font-mono-tab text-text">{fails.length}</span> over
        <span class="text-text">{star}★</span> limits:
        <span class="text-text">{fails.map(labelOf).join(', ')}</span>
      {/if}
    </span>
  </div>
{/snippet}

<div class="mb-8 flex flex-col gap-4">
  <!-- Star verdicts (honest wording) + the estimate disclaimer. -->
  <div class="flex flex-col gap-2">
    <div class="flex flex-wrap items-center gap-2">
      <Badge tone={analysis.two_star_compliant ? 'accent' : 'danger'}>
        2★ {analysis.two_star_compliant ? 'Within limits' : 'Over limits'}
      </Badge>
      <Badge tone={analysis.three_star_compliant ? 'accent' : 'danger'}>
        3★ {analysis.three_star_compliant ? 'Within limits' : 'Over limits'}
      </Badge>
    </div>
    <p class="text-xs text-faint">
      Preflight estimate against the published bet-level limits — the actual star rating is decided
      by Stake Engine, not by this tool.
    </p>
  </div>

  <!-- One-line summary per star. -->
  {#if total > 0}
    <div class="flex flex-col gap-1.5">
      {@render summary(2, fails2)}
      {@render summary(3, fails3)}
    </div>
  {/if}

  <!-- Problems first: only the failing constraints, value → limit, anchored to the row. -->
  {#if failItems.length > 0}
    <div class="rounded-lg border border-danger/30 bg-danger/[0.07] p-4">
      <div class="mb-2.5 flex items-center gap-2 text-sm font-medium text-danger">
        <span aria-hidden="true">▲</span>
        <span>
          {failItems.length} over the estimated limits — jump to a row to see the full gauge
        </span>
      </div>
      <ul class="flex flex-col divide-y divide-danger/10">
        {#each failItems as it (it.key + it.star)}
          <li>
            <a
              href="#c-{it.key}"
              class="group flex items-baseline justify-between gap-3 py-1.5 text-sm"
            >
              <span class="min-w-0 truncate">
                <span
                  class="mr-1.5 rounded border border-danger/30 bg-danger/10 px-1 py-0.5 text-[10px] font-medium text-danger"
                  >{it.star}★</span
                >
                <span class="text-muted transition group-hover:text-text">{it.label}</span>
              </span>
              <span class="flex-none font-mono-tab text-xs">
                <span class="text-text">{formatMetric(it.value)}</span>
                <span class="text-faint">/ {it.limit}</span>
              </span>
            </a>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  <!-- Cross-mode RTP consistency. -->
  <p class="text-sm text-muted">
    Cross-mode RTP consistency:
    <span
      class={analysis.cross_mode_rtp_pass ? 'font-medium text-accent' : 'font-medium text-danger'}
    >
      {analysis.cross_mode_rtp_pass ? 'Consistent' : 'Inconsistent'}
    </span>
    <span class="text-faint">
      · variance <span class="font-mono-tab">{pct(analysis.cross_mode_rtp_variance, 2)}</span>
    </span>
  </p>
</div>
