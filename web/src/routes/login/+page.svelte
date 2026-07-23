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

  type Mode = 'login' | 'register' | 'forgot';

  let mode = $state<Mode>('login');
  let email = $state('');
  let password = $state('');
  let displayName = $state('');
  let busy = $state(false);
  let error = $state('');
  let forgotSent = $state(false);
  let providers = $state<AuthProviders>({ password: true, github: false, discord: false });

  let next = $derived(safeNext(page.url.searchParams.get('next')));
  let hasOAuth = $derived(providers.github || providers.discord);

  onMount(async () => {
    if (session.user) {
      goto(next, { replaceState: true });
      return;
    }
    try {
      providers = await api.auth.providers();
    } catch {
      // Keep defaults (password on, providers off) if the endpoint isn't up yet.
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

  function switchMode(next: Mode) {
    mode = next;
    error = '';
    forgotSent = false;
  }

  async function submit(ev: SubmitEvent) {
    ev.preventDefault();
    if (busy) return;
    error = '';
    busy = true;
    try {
      if (mode === 'forgot') {
        await api.auth.forgotPassword(email.trim());
        forgotSent = true;
        return;
      }
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

  let heading = $derived(
    mode === 'login'
      ? 'Sign in to Stake Dev Tool Cloud'
      : mode === 'register'
        ? 'Create your account'
        : 'Reset your password'
  );
  let subtitle = $derived(
    mode === 'login'
      ? 'Zero-install workbench for slot math teams.'
      : mode === 'register'
        ? 'Start a workspace, invite your team.'
        : 'Enter your email and we’ll send you a reset link.'
  );
</script>

<svelte:head>
  <title>{heading} · Stake Dev Tool Cloud</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center px-6 py-12">
  <div class="fade-in w-full max-w-sm">
    <div class="mb-8 flex flex-col items-center gap-3 text-center">
      <span
        class="flex h-11 w-11 items-center justify-center rounded-xl bg-accent text-lg font-bold text-accent-ink"
        >S</span
      >
      <div>
        <h1 class="text-xl font-semibold tracking-tight">{heading}</h1>
        <p class="mt-1 text-sm text-muted">{subtitle}</p>
      </div>
    </div>

    <Card class="p-6">
      {#if mode === 'forgot' && forgotSent}
        <div class="flex flex-col items-center gap-3 py-2 text-center">
          <span
            class="flex h-11 w-11 items-center justify-center rounded-full bg-accent/10 text-xl text-accent"
            >✉</span
          >
          <h2 class="text-base font-semibold">Check your inbox</h2>
          <p class="text-sm text-muted">
            If an account exists for <span class="text-text">{email.trim()}</span>, we’ve sent a
            link to reset your password. The link expires in one hour.
          </p>
          <button
            type="button"
            onclick={() => switchMode('login')}
            class="mt-1 text-sm font-medium text-accent underline-offset-4 transition hover:underline"
          >
            Back to sign in
          </button>
        </div>
      {:else}
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

          {#if mode !== 'forgot'}
            <div class="flex flex-col gap-1.5">
              <div class="flex items-center justify-between">
                <label for="password" class="text-sm font-medium text-muted">Password</label>
                {#if mode === 'login'}
                  <button
                    type="button"
                    onclick={() => switchMode('forgot')}
                    class="text-xs text-muted underline-offset-4 transition hover:text-text hover:underline"
                  >
                    Forgot password?
                  </button>
                {/if}
              </div>
              <input
                id="password"
                type="password"
                bind:value={password}
                placeholder="••••••••"
                autocomplete={mode === 'login' ? 'current-password' : 'new-password'}
                required
                class="h-9 w-full rounded-md border border-border bg-surface-2 px-3 text-sm text-text outline-none transition placeholder:text-faint focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
              />
            </div>
          {/if}

          {#if error}
            <p
              class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger"
            >
              {error}
            </p>
          {/if}

          <Button type="submit" loading={busy} class="w-full">
            {mode === 'login'
              ? 'Sign in'
              : mode === 'register'
                ? 'Create account'
                : 'Send reset link'}
          </Button>

          {#if mode === 'forgot'}
            <button
              type="button"
              onclick={() => switchMode('login')}
              class="text-center text-sm text-muted underline-offset-4 transition hover:text-text hover:underline"
            >
              Back to sign in
            </button>
          {/if}
        </form>

        {#if mode !== 'forgot' && hasOAuth}
          <div class="my-4 flex items-center gap-3 text-xs text-faint">
            <span class="h-px flex-1 bg-border"></span>
            or
            <span class="h-px flex-1 bg-border"></span>
          </div>
          <div class="flex flex-col gap-2">
            {#if providers.github}
              <Button
                href={api.auth.githubStartUrl()}
                rel="external"
                variant="outline"
                class="w-full"
              >
                <svg viewBox="0 0 16 16" class="h-4 w-4" fill="currentColor" aria-hidden="true">
                  <path
                    d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8z"
                  />
                </svg>
                Continue with GitHub
              </Button>
            {/if}
            {#if providers.discord}
              <Button
                href={api.auth.discordStartUrl()}
                rel="external"
                variant="outline"
                class="w-full"
              >
                <svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor" aria-hidden="true">
                  <path
                    d="M20.317 4.369a19.79 19.79 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.865-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.6 12.6 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994a.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128c.126-.094.252-.192.372-.291a.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.009c.12.099.246.198.373.292a.077.077 0 0 1-.006.127 12.3 12.3 0 0 1-1.873.891.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03zM8.02 15.331c-1.183 0-2.157-1.086-2.157-2.42 0-1.333.955-2.419 2.157-2.419 1.211 0 2.176 1.096 2.157 2.42 0 1.333-.955 2.419-2.157 2.419zm7.975 0c-1.183 0-2.157-1.086-2.157-2.42 0-1.333.955-2.419 2.157-2.419 1.211 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.419-2.157 2.419z"
                  />
                </svg>
                Continue with Discord
              </Button>
            {/if}
          </div>
        {/if}
      {/if}
    </Card>

    {#if mode !== 'forgot'}
      <p class="mt-5 text-center text-sm text-muted">
        {mode === 'login' ? "Don't have an account?" : 'Already have an account?'}
        <button
          type="button"
          onclick={() => switchMode(mode === 'login' ? 'register' : 'login')}
          class="ml-1 font-medium text-accent underline-offset-4 transition hover:underline"
        >
          {mode === 'login' ? 'Create one' : 'Sign in'}
        </button>
      </p>
    {/if}

    <p class="mt-6 text-center text-xs text-faint">
      Independent open-source tool for Stake Engine developers. Not affiliated with Stake.com.
    </p>
  </div>
</main>
