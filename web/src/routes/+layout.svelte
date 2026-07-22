<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { session, refreshSession } from '$lib/session.svelte';
  import Nav from '$lib/components/Nav.svelte';
  import Toasts from '$lib/components/Toasts.svelte';
  import EmailVerifyBanner from '$lib/components/EmailVerifyBanner.svelte';

  let { children } = $props();

  /**
   * /login, /invite/*, /reset/* and /verify/* are reachable without a session.
   * Everything else guards.
   */
  function isPublic(path: string): boolean {
    return (
      path === '/login' ||
      path === '/invite' ||
      path.startsWith('/invite/') ||
      path === '/reset' ||
      path.startsWith('/reset/') ||
      path === '/verify' ||
      path.startsWith('/verify/')
    );
  }

  onMount(refreshSession);

  // Client-side auth guard: once we know who the user is, bounce the
  // unauthenticated away from protected routes, preserving where they wanted to go.
  $effect(() => {
    if (!session.loaded) return;
    const path = page.url.pathname;
    if (!session.user && !isPublic(path)) {
      const next = encodeURIComponent(path + page.url.search);
      goto(`/login?next=${next}`, { replaceState: true });
    }
  });

  let showChrome = $derived(session.loaded && !!session.user && !isPublic(page.url.pathname));
  let ready = $derived(session.loaded || isPublic(page.url.pathname));
</script>

{#if showChrome}
  <Nav />
  <EmailVerifyBanner />
{/if}

{#if ready}
  {@render children()}
{:else}
  <div class="flex min-h-screen items-center justify-center text-muted">
    <span class="spinner"></span>
  </div>
{/if}

<Toasts />
