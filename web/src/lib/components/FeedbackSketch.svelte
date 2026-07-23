<script lang="ts">
  /**
   * FeedbackSketch — renders a visitor's annotation exactly as they drew it:
   * the (optional) screenshot as the backdrop and the vector shapes on top,
   * scaled by viewBox so it stays crisp at any card width. Shape coordinates
   * are CSS pixels of the recorded (viewport_w, viewport_h).
   */
  import type { FeedbackShape } from '$lib/api';

  type Props = {
    shapes: FeedbackShape[];
    /** The visitor's viewport when they drew (drives the viewBox). */
    width: number;
    height: number;
    /** Same-origin screenshot URL, or null (shapes on a dark backdrop). */
    screenshotUrl: string | null;
  };

  let { shapes, width, height, screenshotUrl }: Props = $props();

  function penPoints(p: [number, number][] | undefined): string {
    return (p ?? []).map(([x, y]) => `${x},${y}`).join(' ');
  }

  /** The two arrow-head strokes, mirroring the widget's canvas rendering. */
  function arrowHead(s: FeedbackShape): string {
    const x1 = s.x1 ?? 0;
    const y1 = s.y1 ?? 0;
    const x2 = s.x2 ?? 0;
    const y2 = s.y2 ?? 0;
    const angle = Math.atan2(y2 - y1, x2 - x1);
    const head = Math.max(10, (s.s ?? 3) * 4);
    const a1x = x2 - head * Math.cos(angle - Math.PI / 6);
    const a1y = y2 - head * Math.sin(angle - Math.PI / 6);
    const a2x = x2 - head * Math.cos(angle + Math.PI / 6);
    const a2y = y2 - head * Math.sin(angle + Math.PI / 6);
    return `M ${x2} ${y2} L ${a1x} ${a1y} M ${x2} ${y2} L ${a2x} ${a2y}`;
  }
</script>

<svg
  viewBox={`0 0 ${width} ${height}`}
  class="w-full rounded-md border border-border bg-black/50"
  role="img"
  aria-label="Visitor annotation"
>
  {#if screenshotUrl}
    <image href={screenshotUrl} x="0" y="0" {width} {height} preserveAspectRatio="xMidYMid slice" />
  {/if}
  {#each shapes as s, i (i)}
    {#if s.t === 'pen'}
      <polyline
        points={penPoints(s.p)}
        fill="none"
        stroke={s.c}
        stroke-width={s.s}
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    {:else if s.t === 'rect'}
      <rect x={s.x} y={s.y} width={s.w} height={s.h} fill="none" stroke={s.c} stroke-width={s.s} />
    {:else if s.t === 'ellipse'}
      <ellipse
        cx={(s.x ?? 0) + (s.w ?? 0) / 2}
        cy={(s.y ?? 0) + (s.h ?? 0) / 2}
        rx={Math.abs((s.w ?? 0) / 2)}
        ry={Math.abs((s.h ?? 0) / 2)}
        fill="none"
        stroke={s.c}
        stroke-width={s.s}
      />
    {:else if s.t === 'arrow'}
      <line
        x1={s.x1}
        y1={s.y1}
        x2={s.x2}
        y2={s.y2}
        stroke={s.c}
        stroke-width={s.s}
        stroke-linecap="round"
      />
      <path
        d={arrowHead(s)}
        fill="none"
        stroke={s.c}
        stroke-width={s.s}
        stroke-linecap="round"
      />
    {/if}
  {/each}
</svg>
