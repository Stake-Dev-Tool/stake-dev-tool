<script lang="ts">
  /**
   * WorkspaceSwitcher — a compact Nav dropdown to jump between the user's
   * workspaces from anywhere. Reads the session-cached list (one GET per
   * session), highlights the current workspace (derived from the URL), and
   * offers an "All workspaces" escape hatch to `/`.
   */
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { roleTone } from '$lib/format';
  import { workspacesStore, loadWorkspaces } from '$lib/workspaces.svelte';
  import Badge from '$lib/components/Badge.svelte';

  let open = $state(false);
  let wrapper = $state<HTMLElement>();

  onMount(loadWorkspaces);

  let currentSlug = $derived(page.params.slug ?? '');
  let items = $derived(workspacesStore.items);
  let current = $derived(items.find((m) => m.workspace.slug === currentSlug) ?? null);
  let label = $derived(current?.workspace.name ?? (currentSlug || 'Workspaces'));

  function onWindowClick(e: MouseEvent) {
    if (open && wrapper && !wrapper.contains(e.target as Node)) open = false;
  }
  function onWindowKey(e: KeyboardEvent) {
    if (open && e.key === 'Escape') open = false;
  }
</script>

<svelte:window onclick={onWindowClick} onkeydown={onWindowKey} />

<div bind:this={wrapper} class="relative">
  <button
    type="button"
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={() => (open = !open)}
    class="flex h-8 max-w-[12rem] items-center gap-1.5 rounded-md border border-border bg-surface-2 px-2.5 text-sm text-text transition hover:border-border-strong"
  >
    <span class="h-1.5 w-1.5 flex-shrink-0 rounded-full bg-accent"></span>
    <span class="truncate">{label}</span>
    <svg
      class="h-3.5 w-3.5 flex-shrink-0 text-faint transition {open ? 'rotate-180' : ''}"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="m6 9 6 6 6-6" />
    </svg>
  </button>

  {#if open}
    <div
      role="menu"
      class="fade-in absolute left-0 top-full z-20 mt-1.5 max-h-[70vh] w-64 overflow-y-auto rounded-lg border border-border bg-surface p-1 shadow-xl shadow-black/30"
    >
      <div class="px-2 py-1.5 text-xs font-semibold uppercase tracking-wide text-faint">
        Workspaces
      </div>
      {#if items.length === 0}
        <p class="px-2 py-2 text-sm text-muted">No workspaces yet.</p>
      {:else}
        {#each items as m (m.workspace.id || m.workspace.slug)}
          {@const active = m.workspace.slug === currentSlug}
          <a
            href={`/w/${m.workspace.slug}`}
            role="menuitem"
            onclick={() => (open = false)}
            class="flex items-center justify-between gap-2 rounded-md px-2 py-2 text-sm transition {active
              ? 'bg-surface-2 text-text'
              : 'text-muted hover:bg-surface-2 hover:text-text'}"
          >
            <span class="min-w-0 flex-1">
              <span class="block truncate {active ? 'font-medium' : ''}">{m.workspace.name}</span>
              <span class="block truncate font-mono-tab text-xs text-faint">{m.workspace.slug}</span>
            </span>
            <Badge tone={roleTone(m.role)}>{m.role}</Badge>
          </a>
        {/each}
      {/if}
      <div class="my-1 border-t border-border"></div>
      <a
        href="/"
        role="menuitem"
        onclick={() => (open = false)}
        class="block rounded-md px-2 py-2 text-sm text-muted transition hover:bg-surface-2 hover:text-text"
      >
        All workspaces →
      </a>
    </div>
  {/if}
</div>
