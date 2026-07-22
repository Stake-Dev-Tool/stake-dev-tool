<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api, ApiError } from '$lib/api';
  import { session, refreshSession } from '$lib/session.svelte';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';

  let token = $derived(page.params.token ?? '');
  let status = $state<'checking' | 'ok' | 'invalid'>('checking');

  onMount(async () => {
    try {
      await api.auth.verifyEmail(token);
      status = 'ok';
      // If the user is signed in, refresh so the "verify your email" banner clears.
      if (session.user) await refreshSession();
    } catch (e) {
      if (e instanceof ApiError && e.code === 'invalid_token') {
        status = 'invalid';
      } else {
        // Network or unexpected error — treat as invalid so the user can retry.
        status = 'invalid';
      }
    }
  });
</script>

<svelte:head><title>Verify email · Stake Dev Tool Cloud</title></svelte:head>

<main class="flex min-h-screen items-center justify-center px-6 py-12">
  <div class="fade-in w-full max-w-sm">
    <div class="mb-8 flex justify-center">
      <a href="/" class="flex items-center gap-2.5">
        <span
          class="flex h-8 w-8 items-center justify-center rounded-lg bg-accent text-sm font-bold text-accent-ink"
          >S</span
        >
        <span class="font-semibold tracking-tight">Stake Dev Tool Cloud</span>
      </a>
    </div>

    <Card class="p-6">
      {#if status === 'checking'}
        <div class="flex flex-col items-center gap-3 py-4 text-center">
          <span class="spinner"></span>
          <p class="text-sm text-muted">Verifying your email…</p>
        </div>
      {:else if status === 'ok'}
        <div class="flex flex-col items-center gap-3 py-2 text-center">
          <span
            class="flex h-11 w-11 items-center justify-center rounded-full bg-ok/10 text-xl text-ok"
            >✓</span
          >
          <h1 class="text-base font-semibold">Email verified</h1>
          <p class="text-sm text-muted">Your email address is confirmed. You’re all set.</p>
          {#if session.user}
            <Button href="/" class="mt-1 w-full">Continue to dashboard</Button>
          {:else}
            <Button href="/login" class="mt-1 w-full">Sign in</Button>
          {/if}
        </div>
      {:else}
        <div class="flex flex-col items-center gap-3 py-2 text-center">
          <span
            class="flex h-11 w-11 items-center justify-center rounded-full bg-danger/10 text-xl text-danger"
            >!</span
          >
          <h1 class="text-base font-semibold">This link is no longer valid</h1>
          <p class="text-sm text-muted">
            Verification links expire after 24 hours and can be used only once. Sign in and request a
            fresh link from the banner at the top of your dashboard.
          </p>
          <Button href="/login" variant="outline" class="mt-1 w-full">Sign in</Button>
        </div>
      {/if}
    </Card>
  </div>
</main>
