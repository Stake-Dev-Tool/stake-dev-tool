<script lang="ts">
  import { page } from '$app/state';
  import { api, ApiError } from '$lib/api';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';

  let token = $derived(page.params.token ?? '');

  let password = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let error = $state('');
  let done = $state(false);
  let invalidToken = $state(false);

  const MIN = 8;
  let tooShort = $derived(password.length > 0 && password.length < MIN);
  let mismatch = $derived(confirm.length > 0 && confirm !== password);
  let canSubmit = $derived(password.length >= MIN && confirm === password && !busy);

  async function submit(ev: SubmitEvent) {
    ev.preventDefault();
    if (!canSubmit) return;
    error = '';
    busy = true;
    try {
      await api.auth.resetPassword(token, password);
      done = true;
    } catch (e) {
      if (e instanceof ApiError && e.code === 'invalid_token') {
        invalidToken = true;
      } else if (e instanceof ApiError && e.code === 'weak_password') {
        error = `Password must be at least ${MIN} characters.`;
      } else if (e instanceof ApiError) {
        error = e.message || 'Something went wrong.';
      } else {
        error = 'Something went wrong.';
      }
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head><title>Reset password · Stake Dev Tool Cloud</title></svelte:head>

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
      {#if done}
        <div class="flex flex-col items-center gap-3 py-2 text-center">
          <span
            class="flex h-11 w-11 items-center justify-center rounded-full bg-ok/10 text-xl text-ok"
            >✓</span
          >
          <h1 class="text-base font-semibold">Password updated</h1>
          <p class="text-sm text-muted">
            Your password has been changed and every existing session was signed out. Sign in with
            your new password.
          </p>
          <Button href="/login" class="mt-1 w-full">Sign in</Button>
        </div>
      {:else if invalidToken}
        <div class="flex flex-col items-center gap-3 py-2 text-center">
          <span
            class="flex h-11 w-11 items-center justify-center rounded-full bg-danger/10 text-xl text-danger"
            >!</span
          >
          <h1 class="text-base font-semibold">This link is no longer valid</h1>
          <p class="text-sm text-muted">
            Password reset links expire after one hour and can be used only once. Request a fresh
            one from the sign-in page.
          </p>
          <Button href="/login" variant="outline" class="mt-1 w-full">Request a new link</Button>
        </div>
      {:else}
        <div class="mb-4 text-center">
          <h1 class="text-lg font-semibold tracking-tight">Choose a new password</h1>
          <p class="mt-1 text-sm text-muted">Set a new password for your account.</p>
        </div>
        <form class="flex flex-col gap-4" onsubmit={submit}>
          <Input
            id="password"
            label="New password"
            type="password"
            bind:value={password}
            placeholder="••••••••"
            autocomplete="new-password"
            required
            error={tooShort ? `At least ${MIN} characters.` : undefined}
          />
          <Input
            id="confirm"
            label="Confirm password"
            type="password"
            bind:value={confirm}
            placeholder="••••••••"
            autocomplete="new-password"
            required
            error={mismatch ? 'Passwords don’t match.' : undefined}
          />

          {#if error}
            <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
              {error}
            </p>
          {/if}

          <Button type="submit" loading={busy} disabled={!canSubmit} class="w-full">
            Update password
          </Button>
        </form>
      {/if}
    </Card>
  </div>
</main>
