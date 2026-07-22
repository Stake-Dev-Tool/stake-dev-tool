<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { api, ApiError, type AuthProviders } from '$lib/api';
  import { session, setUser } from '$lib/session.svelte';
  import { safeNext } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';

  let mode = $state<'login' | 'register'>('login');
  let email = $state('');
  let password = $state('');
  let displayName = $state('');
  let busy = $state(false);
  let error = $state('');
  let providers = $state<AuthProviders>({ password: true, github: false });

  let next = $derived(safeNext(page.url.searchParams.get('next')));

  onMount(async () => {
    if (session.user) {
      goto(next, { replaceState: true });
      return;
    }
    try {
      providers = await api.auth.providers();
    } catch {
      // Keep defaults (password on, github off) if the endpoint isn't up yet.
    }
  });

  function messageFor(e: unknown): string {
    if (e instanceof ApiError) {
      switch (e.code) {
        case 'invalid_credentials':
          return 'Incorrect email or password.';
        case 'email_taken':
          return 'That email is already registered — try signing in instead.';
        default:
          if (e.status === 429) return 'Too many attempts. Please wait a moment and try again.';
          if (e.status === 0) return e.message;
          return e.message || 'Something went wrong.';
      }
    }
    return 'Something went wrong.';
  }

  function toggleMode() {
    mode = mode === 'login' ? 'register' : 'login';
    error = '';
  }

  async function submit(ev: SubmitEvent) {
    ev.preventDefault();
    if (busy) return;
    error = '';
    busy = true;
    try {
      const user =
        mode === 'login'
          ? await api.auth.login(email.trim(), password)
          : await api.auth.register(email.trim(), password, displayName.trim());
      setUser(user);
      await goto(next, { replaceState: true });
    } catch (e) {
      error = messageFor(e);
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head>
  <title>{mode === 'login' ? 'Sign in' : 'Create account'} · Stake Dev Tool Cloud</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center px-6 py-12">
  <div class="fade-in w-full max-w-sm">
    <div class="mb-8 flex flex-col items-center gap-3 text-center">
      <span
        class="flex h-11 w-11 items-center justify-center rounded-xl bg-accent text-lg font-bold text-accent-ink"
        >S</span
      >
      <div>
        <h1 class="text-xl font-semibold tracking-tight">
          {mode === 'login' ? 'Sign in to Stake Dev Tool Cloud' : 'Create your account'}
        </h1>
        <p class="mt-1 text-sm text-muted">
          {mode === 'login'
            ? 'Zero-install workbench for slot math teams.'
            : 'Start a workspace, invite your team.'}
        </p>
      </div>
    </div>

    <Card class="p-6">
      <form class="flex flex-col gap-4" onsubmit={submit}>
        {#if mode === 'register'}
          <Input
            id="display_name"
            label="Display name"
            bind:value={displayName}
            placeholder="Ada Lovelace"
            autocomplete="name"
            required
          />
        {/if}

        <Input
          id="email"
          label="Email"
          type="email"
          bind:value={email}
          placeholder="you@studio.com"
          autocomplete="email"
          required
        />

        <Input
          id="password"
          label="Password"
          type="password"
          bind:value={password}
          placeholder="••••••••"
          autocomplete={mode === 'login' ? 'current-password' : 'new-password'}
          required
        />

        {#if error}
          <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
            {error}
          </p>
        {/if}

        <Button type="submit" loading={busy} class="w-full">
          {mode === 'login' ? 'Sign in' : 'Create account'}
        </Button>
      </form>

      {#if providers.github}
        <div class="my-4 flex items-center gap-3 text-xs text-faint">
          <span class="h-px flex-1 bg-border"></span>
          or
          <span class="h-px flex-1 bg-border"></span>
        </div>
        <Button href={api.auth.githubStartUrl()} rel="external" variant="outline" class="w-full">
          Continue with GitHub
        </Button>
      {/if}
    </Card>

    <p class="mt-5 text-center text-sm text-muted">
      {mode === 'login' ? "Don't have an account?" : 'Already have an account?'}
      <button
        type="button"
        onclick={toggleMode}
        class="ml-1 font-medium text-accent underline-offset-4 transition hover:underline"
      >
        {mode === 'login' ? 'Create one' : 'Sign in'}
      </button>
    </p>
  </div>
</main>
