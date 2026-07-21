<script lang="ts">
  import { page } from '$app/state';
  import { session } from '$lib/session.svelte';

  const links = [
    { href: '/', label: 'Workspaces' },
    { href: '/device', label: 'Device' },
    { href: '/account', label: 'Account' }
  ];

  function isActive(href: string, path: string): boolean {
    if (href === '/') return path === '/' || path.startsWith('/w/');
    return path === href || path.startsWith(href + '/');
  }
</script>

<header class="sticky top-0 z-10 border-b border-border bg-bg/80 backdrop-blur">
  <div class="mx-auto flex h-14 w-full max-w-5xl items-center justify-between gap-4 px-6">
    <a href="/" class="flex items-center gap-2.5">
      <span
        class="flex h-7 w-7 items-center justify-center rounded-md bg-accent text-sm font-bold text-accent-ink"
        >S</span
      >
      <span class="text-sm font-semibold tracking-tight">Stake Cloud</span>
    </a>
    <nav class="flex items-center gap-1">
      {#each links as l (l.href)}
        <a
          href={l.href}
          class="rounded-md px-3 py-1.5 text-sm transition {isActive(l.href, page.url.pathname)
            ? 'bg-surface-2 text-text'
            : 'text-muted hover:text-text'}"
        >
          {l.label}
        </a>
      {/each}
      {#if session.user}
        <a
          href="/account"
          class="ml-2 max-w-[12rem] truncate border-l border-border pl-3 text-sm text-muted transition hover:text-text"
          title={session.user.email}
        >
          {session.user.display_name || session.user.email}
        </a>
      {/if}
    </nav>
  </div>
</header>
