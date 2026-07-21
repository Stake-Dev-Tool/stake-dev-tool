<script lang="ts">
  import { flagUrl } from '$lib/api.http';

  type Item = {
    code: string;
    name: string;
    country?: string | null;
    symbol?: string;
    badge?: string;
  };

  let {
    items,
    value,
    onSelect,
    label = 'Select…'
  }: {
    items: Item[];
    value: string;
    onSelect: (code: string) => void;
    label?: string;
  } = $props();

  let open = $state(false);
  let query = $state('');
  let trigger = $state<HTMLButtonElement | null>(null);
  let panel = $state<HTMLDivElement | null>(null);
  let panelTop = $state(0);
  let panelLeft = $state(0);
  let panelWidth = $state(260);

  /** Move node to document.body so it escapes any ancestor that establishes a
   *  containing block for fixed positioning (transform, filter, backdrop-filter,
   *  will-change, contain…). Bulletproof against parent CSS. */
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        if (node.parentNode) node.parentNode.removeChild(node);
      }
    };
  }

  const current = $derived(items.find((i) => i.code === value));
  const filtered = $derived(
    query
      ? items.filter(
          (i) =>
            i.code.toLowerCase().includes(query.toLowerCase()) ||
            i.name.toLowerCase().includes(query.toLowerCase())
        )
      : items
  );

  function handleClickOutside(e: MouseEvent) {
    if (!open) return;
    const t = e.target as Node;
    if (trigger?.contains(t)) return;
    if (panel?.contains(t)) return;
    open = false;
  }

  function handleKey(e: KeyboardEvent) {
    if (e.key === 'Escape') open = false;
  }

  function pick(code: string) {
    onSelect(code);
    open = false;
    query = '';
  }

  function computePosition() {
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const margin = 8;
    const height = 320;

    // Width cannot exceed viewport. Aim for ≥220px but clamp to available space.
    const maxWidth = window.innerWidth - 2 * margin;
    const width = Math.min(Math.max(rect.width, 220), maxWidth);

    // Vertical: open below by default, flip above if not enough space.
    const spaceBelow = window.innerHeight - rect.bottom;
    const openAbove = spaceBelow < height + 12 && rect.top > height;
    panelTop = openAbove ? rect.top - height - 6 : rect.bottom + 6;

    // Horizontal: right-align to trigger, then clamp BOTH sides within viewport.
    let left = rect.right - width;
    if (left + width > window.innerWidth - margin) {
      left = window.innerWidth - width - margin;
    }
    if (left < margin) left = margin;

    panelLeft = left;
    panelWidth = width;
  }

  function toggleOpen() {
    if (open) {
      open = false;
      return;
    }
    computePosition();
    open = true;
  }

  $effect(() => {
    if (open) {
      document.addEventListener('mousedown', handleClickOutside);
      document.addEventListener('keydown', handleKey);
      window.addEventListener('resize', computePosition);
      window.addEventListener('scroll', computePosition, true);
      return () => {
        document.removeEventListener('mousedown', handleClickOutside);
        document.removeEventListener('keydown', handleKey);
        window.removeEventListener('resize', computePosition);
        window.removeEventListener('scroll', computePosition, true);
      };
    }
  });
</script>

<div class="relative min-w-0 w-full">
  <button
    type="button"
    bind:this={trigger}
    onclick={toggleOpen}
    class="flex w-full min-w-0 items-center justify-between gap-1.5 overflow-hidden rounded-md border border-zinc-800 bg-zinc-950/60 px-2 py-1.5 text-left text-sm transition hover:border-zinc-700 focus:border-emerald-500/50 focus:outline-none focus:ring-2 focus:ring-emerald-500/20"
  >
    {#if current}
      <span class="flex min-w-0 items-center gap-2">
        {#if current.country}
          <img
            src={flagUrl(current.country, 20)}
            alt={current.country}
            class="h-3 w-[18px] flex-shrink-0 rounded-sm object-cover ring-1 ring-zinc-700/50"
            loading="lazy"
          />
        {:else if current.badge}
          <span
            class="flex h-3 w-[18px] flex-shrink-0 items-center justify-center rounded-sm bg-amber-500/20 text-[8px] font-bold text-amber-300 ring-1 ring-amber-500/30"
          >
            {current.badge}
          </span>
        {/if}
        <span class="truncate font-mono text-xs uppercase tracking-wide text-zinc-100">
          {current.code}
        </span>
      </span>
    {:else}
      <span class="text-xs text-zinc-500">{label}</span>
    {/if}
    <svg
      class="h-3 w-3 flex-shrink-0 text-zinc-500 transition {open ? 'rotate-180' : ''}"
      fill="none"
      stroke="currentColor"
      stroke-width="2.5"
      viewBox="0 0 24 24"
    >
      <path stroke-linecap="round" stroke-linejoin="round" d="M19 9l-7 7-7-7" />
    </svg>
  </button>
</div>

{#if open}
  <div
    bind:this={panel}
    use:portal
    class="fixed z-[100] overflow-hidden rounded-lg border border-zinc-700/80 bg-zinc-900/95 shadow-2xl"
    style="top: {panelTop}px; left: {panelLeft}px; width: {panelWidth}px; max-width: calc(100vw - 16px); max-height: 320px;"
  >
    <div class="border-b border-zinc-800 p-1.5">
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="text"
        name="picker-search"
        aria-label="Search"
        bind:value={query}
        placeholder="Search…"
        autofocus
        class="w-full rounded-md border border-transparent bg-zinc-950/60 px-2 py-1 text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-emerald-500/40 focus:outline-none"
      />
    </div>
    <div class="max-h-60 overflow-y-auto py-1">
      {#each filtered as item (item.code)}
        {@const selected = item.code === value}
        <button
          type="button"
          onclick={() => pick(item.code)}
          class="flex w-full items-center gap-2.5 px-2.5 py-1.5 text-left text-xs transition hover:bg-zinc-800/70 {selected
            ? 'bg-emerald-500/10 text-emerald-300'
            : 'text-zinc-300'}"
        >
          {#if item.country}
            <img
              src={flagUrl(item.country, 20)}
              alt={item.country}
              class="h-3 w-[18px] flex-shrink-0 rounded-sm object-cover ring-1 ring-zinc-700/50"
              loading="lazy"
            />
          {:else if item.badge}
            <span
              class="flex h-3 w-[18px] flex-shrink-0 items-center justify-center rounded-sm bg-amber-500/20 text-[8px] font-bold text-amber-300 ring-1 ring-amber-500/30"
            >
              {item.badge}
            </span>
          {:else}
            <span class="h-3 w-[18px] flex-shrink-0"></span>
          {/if}
          <span class="w-10 flex-shrink-0 font-mono uppercase tracking-wide">{item.code}</span>
          <span class="truncate text-zinc-500">{item.name}</span>
          {#if item.symbol}
            <span class="ml-auto flex-shrink-0 text-[10px] text-zinc-600">{item.symbol}</span>
          {/if}
        </button>
      {:else}
        <div class="px-3 py-4 text-center text-xs text-zinc-600">No match</div>
      {/each}
    </div>
  </div>
{/if}
