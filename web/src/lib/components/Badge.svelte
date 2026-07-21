<script lang="ts">
  import type { Snippet } from 'svelte';

  type Tone = 'neutral' | 'accent' | 'info' | 'danger' | 'warn';

  let {
    tone = 'neutral',
    class: klass = '',
    children
  }: { tone?: Tone; class?: string; children?: Snippet } = $props();

  const tones: Record<Tone, string> = {
    neutral: 'border-border bg-surface-2 text-muted',
    accent: 'border-accent/30 bg-accent/10 text-accent',
    info: 'border-info/30 bg-info/10 text-info',
    danger: 'border-danger/30 bg-danger/10 text-danger',
    warn: 'border-warn/30 bg-warn/10 text-warn'
  };
</script>

<span
  class="inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium {tones[
    tone
  ]} {klass}"
>
  {@render children?.()}
</span>
