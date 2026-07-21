<script lang="ts">
  /**
   * SectionHeader — the consistent section heading used across pages: a small
   * uppercase h2 eyebrow, an optional muted description (the `children` snippet,
   * so it can carry inline code), and an optional right-aligned `action` slot
   * (a button, a link…). Keeps every section's header visually identical.
   */
  import type { Snippet } from 'svelte';

  type Props = { title: string; class?: string; action?: Snippet; children?: Snippet };
  let { title, class: klass = '', action, children }: Props = $props();
</script>

<div class="mb-3 flex flex-wrap items-start justify-between gap-x-4 gap-y-2 {klass}">
  <div class="min-w-0">
    <h2 class="text-sm font-semibold uppercase tracking-wide text-faint">{title}</h2>
    {#if children}
      <div class="mt-1 text-sm text-muted">{@render children()}</div>
    {/if}
  </div>
  {#if action}
    <div class="flex shrink-0 items-center gap-2">{@render action()}</div>
  {/if}
</div>
