<script lang="ts">
  /**
   * Breadcrumbs — the small, clickable context trail shown above a page's h1
   * (e.g. `workspace / game / rev 3`). The last item is the current page and is
   * rendered as plain text; every earlier item links up a level.
   */
  export type Crumb = { label: string; href?: string };

  type Props = { items: Crumb[]; class?: string };
  let { items, class: klass = '' }: Props = $props();
</script>

<nav aria-label="Breadcrumb" class="mb-4 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-sm {klass}">
  {#each items as item, i (i)}
    {#if i > 0}<span class="text-faint" aria-hidden="true">/</span>{/if}
    {#if item.href && i < items.length - 1}
      <a
        href={item.href}
        class="max-w-[14rem] truncate text-muted transition hover:text-text">{item.label}</a
      >
    {:else}
      <span class="max-w-[16rem] truncate text-text" aria-current="page">{item.label}</span>
    {/if}
  {/each}
</nav>
