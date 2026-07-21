<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api, type ApiToken, type CreatedToken, type TokenScope } from '$lib/api';
  import { session, setUser } from '$lib/session.svelte';
  import { formatDate, formatExpiry, errorText } from '$lib/format';
  import { resetWorkspaces } from '$lib/workspaces.svelte';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';

  const SCOPES: { id: TokenScope; label: string; desc: string }[] = [
    { id: 'full', label: 'full', desc: 'Full access to your workspaces and their data.' },
    { id: 'push:math', label: 'push:math', desc: 'Push math revisions from CI. Nothing else.' }
  ];

  let tokens = $state<ApiToken[]>([]);
  let loading = $state(true);
  let loadError = $state('');
  let actionError = $state('');

  // Create form
  let name = $state('');
  let scopes = $state<Record<TokenScope, boolean>>({ full: false, 'push:math': true });
  let expiry = $state('90'); // days, blank = never
  let creating = $state(false);
  let createError = $state('');
  let revealed = $state<CreatedToken | null>(null);

  let selectedScopes = $derived(SCOPES.filter((s) => scopes[s.id]).map((s) => s.id));
  let canCreate = $derived(name.trim().length > 0 && selectedScopes.length > 0 && !creating);

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      tokens = await api.tokens.list();
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  async function create(ev: SubmitEvent) {
    ev.preventDefault();
    if (!canCreate) return;
    creating = true;
    createError = '';
    revealed = null;
    try {
      const days = expiry.trim() === '' ? undefined : Number(expiry);
      revealed = await api.tokens.create({
        name: name.trim(),
        scopes: selectedScopes,
        expires_in_days: Number.isFinite(days as number) ? days : undefined
      });
      name = '';
      await load();
    } catch (e) {
      createError = errorText(e);
    } finally {
      creating = false;
    }
  }

  async function revoke(t: ApiToken) {
    if (!confirm(`Revoke token "${t.name}"? Anything using it stops working immediately.`)) return;
    actionError = '';
    try {
      await api.tokens.remove(t.id);
      await load();
    } catch (e) {
      actionError = errorText(e);
    }
  }

  async function logout() {
    try {
      await api.auth.logout();
    } catch {
      // Even if the network call fails, drop local state and go to login.
    }
    setUser(null);
    resetWorkspaces();
    await goto('/login');
  }
</script>

<svelte:head><title>Account · Stake Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-3xl px-6 py-10">
  <div class="mb-8 flex items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight">Account</h1>
      {#if session.user}
        <p class="mt-1 text-sm text-muted">
          {session.user.display_name}
          <span class="text-faint">· {session.user.email}</span>
        </p>
      {/if}
    </div>
    <Button variant="outline" onclick={logout}>Log out</Button>
  </div>

  <section>
    <SectionHeader title="API tokens">
      For CI pipelines and the CLI. Scope them down — <span class="font-mono-tab text-text"
        >push:math</span
      > is all a math-push job needs.
    </SectionHeader>

    <Card class="mb-4 p-6">
      <form class="flex flex-col gap-4" onsubmit={create}>
        <Input id="tok-name" label="Name" bind:value={name} placeholder="ci-math-push" required />

        <div class="flex flex-col gap-2">
          <span class="text-sm font-medium text-muted">Scopes</span>
          {#each SCOPES as s (s.id)}
            <label
              class="flex cursor-pointer items-start gap-3 rounded-md border border-border bg-surface-2 px-3 py-2.5 transition hover:border-border-strong"
            >
              <input
                type="checkbox"
                checked={scopes[s.id]}
                onchange={(e) => (scopes[s.id] = (e.currentTarget as HTMLInputElement).checked)}
                class="mt-0.5 h-4 w-4 accent-accent"
              />
              <span>
                <span class="font-mono-tab text-sm text-text">{s.label}</span>
                <span class="mt-0.5 block text-xs text-faint">{s.desc}</span>
              </span>
            </label>
          {/each}
        </div>

        <Input
          id="tok-expiry"
          label="Expires in (days)"
          type="number"
          min="1"
          bind:value={expiry}
          placeholder="never"
          hint="Blank = never expires"
          class="max-w-xs"
        />

        {#if createError}
          <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
            {createError}
          </p>
        {/if}

        <div><Button type="submit" loading={creating} disabled={!canCreate}>Create token</Button></div>
      </form>

      {#if revealed}
        <div class="fade-in mt-5 rounded-md border border-accent/30 bg-accent/5 p-4">
          <div class="mb-2 flex items-center gap-2">
            <Badge tone="accent">New token</Badge>
            <span class="text-xs text-warn">Copy it now — you won't see this secret again.</span>
          </div>
          <CopyField label={revealed.name} value={revealed.token} />
        </div>
      {/if}
    </Card>

    {#if actionError}
      <p class="mb-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
        {actionError}
      </p>
    {/if}

    <Card class="overflow-hidden">
      {#if loading}
        <div class="p-4"><Skeleton /></div>
      {:else if loadError}
        <div class="px-4 py-6">
          <p class="text-sm text-danger">{loadError}</p>
          <Button variant="outline" size="sm" class="mt-3" onclick={load}>Retry</Button>
        </div>
      {:else if tokens.length === 0}
        <p class="px-4 py-8 text-center text-sm text-muted">No tokens yet.</p>
      {:else}
        <div class="overflow-x-auto">
          <table class="w-full min-w-[38rem] text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                <th class="px-4 py-3 font-medium">Name</th>
                <th class="px-4 py-3 font-medium">Scopes</th>
                <th class="px-4 py-3 font-medium">Created</th>
                <th class="px-4 py-3 font-medium">Expires</th>
                <th class="px-4 py-3 font-medium text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {#each tokens as t (t.id)}
                <tr class="border-b border-border/60 last:border-0">
                  <td class="px-4 py-3 font-medium">{t.name}</td>
                  <td class="px-4 py-3">
                    <div class="flex flex-wrap gap-1">
                      {#each t.scopes as sc (sc)}
                        <Badge>{sc}</Badge>
                      {/each}
                    </div>
                  </td>
                  <td class="px-4 py-3 text-muted">{formatDate(t.created_at)}</td>
                  <td class="px-4 py-3 text-muted">{formatExpiry(t.expires_at)}</td>
                  <td class="px-4 py-3 text-right">
                    <Button variant="danger" size="sm" onclick={() => revoke(t)}>Revoke</Button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </Card>
  </section>
</main>
