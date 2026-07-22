<script lang="ts">
  /**
   * Sparkline — a tiny dependency-free bar chart for a 30-day daily series
   * (signups, pushes). Pure inline SVG: one accent-filled `<rect>` per day with
   * a per-day `<title>` tooltip, stretched to fill its container width
   * (`preserveAspectRatio="none"`). Renders a calm "no activity" panel when the
   * series is empty or every day is zero, so it never shows a blank axis.
   */
  import type { AdminDayCount } from '$lib/api';
  import { formatDate } from '$lib/format';

  type Props = {
    data: AdminDayCount[];
    /** Noun for the tooltip/aria ("signups", "pushes"). */
    label?: string;
    class?: string;
  };
  let { data, label = '', class: klass = '' }: Props = $props();

  /** viewBox height in user units (element is `h-12` = 48px; y-scale ≈ 1:1). */
  const VH = 48;

  let hasData = $derived(data.length > 0 && data.some((d) => d.count > 0));
  let max = $derived(data.reduce((m, d) => Math.max(m, d.count), 0) || 1);
  let cols = $derived(data.length || 1);

  /** Bar geometry for a day: a small floor keeps a single event visible. */
  function bar(count: number): { y: number; h: number } {
    if (count <= 0) return { y: VH, h: 0 };
    const h = Math.max(2, (count / max) * (VH - 2));
    return { y: VH - h, h };
  }

  function tip(d: AdminDayCount): string {
    const n = d.count.toLocaleString();
    return `${formatDate(d.date)} — ${n}${label ? ` ${label}` : ''}`;
  }
</script>

{#if !hasData}
  <div
    class="flex h-12 items-center justify-center rounded-md border border-border/60 bg-surface-2 text-xs text-faint {klass}"
  >
    No {label || 'activity'} in the last 30 days
  </div>
{:else}
  <svg
    class="block h-12 w-full {klass}"
    viewBox={`0 0 ${cols} ${VH}`}
    preserveAspectRatio="none"
    role="img"
    aria-label={`${label || 'Activity'} over the last ${data.length} days`}
  >
    {#each data as d, i (d.date || i)}
      {@const b = bar(d.count)}
      <rect x={i + 0.12} y={b.y} width={0.76} height={b.h} class="fill-accent">
        <title>{tip(d)}</title>
      </rect>
    {/each}
  </svg>
{/if}
