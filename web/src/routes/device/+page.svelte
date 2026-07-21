<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api, ApiError } from '$lib/api';
  import { errorText } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';

  // Codes look like ABCD-EFGH. We store the formatted value and normalize on the fly.
  let code = $state('');
  let busy = $state<false | 'approve' | 'deny'>(false);
  let result = $state<'approved' | 'denied' | null>(null);
  let error = $state('');

  let normalized = $derived(code.replace(/[^A-Z0-9]/g, ''));
  let complete = $derived(normalized.length === 8);

  onMount(() => {
    const prefill = page.url.searchParams.get('user_code') ?? page.url.searchParams.get('code');
    if (prefill) code = format(prefill);
  });

  function format(raw: string): string {
    const v = raw.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 8);
    return v.length > 4 ? `${v.slice(0, 4)}-${v.slice(4)}` : v;
  }

  function onInput(e: Event) {
    code = format((e.currentTarget as HTMLInputElement).value);
  }

  async function decide(approve: boolean) {
    if (!complete || busy) return;
    error = '';
    result = null;
    busy = approve ? 'approve' : 'deny';
    try {
      await api.device.approve(code, approve);
      result = approve ? 'approved' : 'denied';
    } catch (e) {
      if (e instanceof ApiError && (e.code === 'invalid_code' || e.status === 404)) {
        error = "That code wasn't recognized. Check it and try again — codes expire quickly.";
      } else {
        error = errorText(e);
      }
    } finally {
      busy = false;
    }
  }

  function reset() {
    result = null;
    error = '';
    code = '';
  }
</script>

<svelte:head><title>Approve device · Stake Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-md px-6 py-16">
  <div class="mb-6 text-center">
    <h1 class="text-xl font-semibold tracking-tight">Approve a device</h1>
    <p class="mt-1.5 text-sm text-muted">
      Enter the code shown by the desktop app or CLI to link it to your account.
    </p>
  </div>

  <Card class="p-6">
    {#if result === 'approved'}
      <div class="flex flex-col items-center gap-3 py-4 text-center">
        <span class="flex h-11 w-11 items-center justify-center rounded-full bg-accent/10 text-accent text-xl">✓</span>
        <h2 class="text-lg font-semibold">Device approved</h2>
        <p class="text-sm text-muted">You can return to the app — it's now signed in.</p>
        <Button variant="outline" class="mt-2" onclick={reset}>Approve another</Button>
      </div>
    {:else if result === 'denied'}
      <div class="flex flex-col items-center gap-3 py-4 text-center">
        <span class="flex h-11 w-11 items-center justify-center rounded-full bg-surface-2 text-muted text-xl">✕</span>
        <h2 class="text-lg font-semibold">Request denied</h2>
        <p class="text-sm text-muted">The device was not granted access.</p>
        <Button variant="outline" class="mt-2" onclick={reset}>Enter another code</Button>
      </div>
    {:else}
      <label for="device-code" class="text-sm font-medium text-muted">Device code</label>
      <input
        id="device-code"
        value={code}
        oninput={onInput}
        inputmode="text"
        autocapitalize="characters"
        autocomplete="off"
        spellcheck="false"
        placeholder="ABCD-EFGH"
        aria-label="Device code"
        class="mt-1.5 h-14 w-full rounded-md border border-border bg-surface-2 text-center font-mono-tab text-2xl uppercase tracking-[0.35em] text-text outline-none transition placeholder:text-faint placeholder:tracking-[0.2em] focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
      />

      {#if error}
        <p class="mt-3 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
          {error}
        </p>
      {/if}

      <div class="mt-5 flex gap-3">
        <Button class="flex-1" disabled={!complete} loading={busy === 'approve'} onclick={() => decide(true)}>
          Approve
        </Button>
        <Button
          variant="outline"
          class="flex-1"
          disabled={!complete}
          loading={busy === 'deny'}
          onclick={() => decide(false)}
        >
          Deny
        </Button>
      </div>
    {/if}
  </Card>
</main>
