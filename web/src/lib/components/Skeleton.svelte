<script lang="ts">
  /**
   * Skeleton — a few shimmering placeholder lines for page-level loading, shown
   * inside a Card in place of a centered spinner. Button-level loading keeps its
   * own spinner; this is only for "the page's data is on its way".
   */
  type Props = { lines?: number; class?: string };
  let { lines = 3, class: klass = '' }: Props = $props();

  const widths = ['100%', '92%', '75%', '85%', '68%'];
</script>

<div class="flex w-full flex-col gap-3 {klass}" aria-hidden="true">
  {#each Array(lines) as _, i (i)}
    <div class="skeleton h-4 rounded" style="width: {widths[i % widths.length]}"></div>
  {/each}
</div>
