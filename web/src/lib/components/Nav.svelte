<script lang="ts">
  import { page } from '$app/state';
  import { session } from '$lib/session.svelte';
  import WorkspaceSwitcher from '$lib/components/WorkspaceSwitcher.svelte';

  const links = [
    { href: '/device', label: 'Device' },
    { href: '/account', label: 'Account' }
  ];

  function isActive(href: string, path: string): boolean {
    return path === href || path.startsWith(href + '/');
  }
</script>

<header class="sticky top-0 z-10 border-b border-border bg-bg/80 backdrop-blur">
  <div
    class="mx-auto flex w-full max-w-5xl flex-wrap items-center gap-x-4 gap-y-2 px-6 py-2.5 sm:min-h-14"
  >
    <a href="/" class="flex flex-shrink-0 items-center gap-2.5">
      <span
        class="flex h-7 w-7 items-center justify-center rounded-md bg-accent text-sm font-bold text-accent-ink"
        >S</span
      >
      <span class="text-sm font-semibold tracking-tight">Stake Cloud</span>
    </a>

    {#if session.user}
      <WorkspaceSwitcher />
    {/if}

    <nav class="ml-auto flex flex-wrap items-center gap-1">
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
          class="ml-1 max-w-[12rem] truncate border-l border-border pl-3 text-sm text-muted transition hover:text-text"
          title={session.user.email}
        >
          {session.user.display_name || session.user.email}
        </a>
      {/if}
    </nav>
  </div>
</header>
