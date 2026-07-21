<script lang="ts">
  /**
   * EmptyState — the one shared "nothing here yet" panel: a dashed Card with a
   * title, muted helper copy (the `children` snippet, so it can carry inline
   * code/links), and at most one call-to-action. Deliberately icon-less so every
   * empty state across the app reads identically.
   */
  import type { Snippet } from 'svelte';
  import Card from '$lib/components/Card.svelte';

  type Props = { title: string; class?: string; children?: Snippet; cta?: Snippet };
  let { title, class: klass = '', children, cta }: Props = $props();
</script>

<Card class="flex flex-col items-center gap-3 border-dashed px-6 py-14 text-center {klass}">
  <h3 class="text-base font-semibold">{title}</h3>
  {#if children}
    <div class="max-w-md text-sm leading-relaxed text-muted">{@render children()}</div>
  {/if}
  {#if cta}
    <div class="mt-1">{@render cta()}</div>
  {/if}
</Card>
