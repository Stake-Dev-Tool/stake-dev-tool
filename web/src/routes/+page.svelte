<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import {
    api,
    ApiError,
    isValidSlug,
    slugFromName,
    type WorkspaceMembership
  } from '$lib/api';
  import { roleTone, errorText } from '$lib/format';
  import { setWorkspaces, loadWorkspaces } from '$lib/workspaces.svelte';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';

  let memberships = $state<WorkspaceMembership[]>([]);
  let loading = $state(true);
  let loadError = $state('');

  // Create-workspace form
  let showCreate = $state(false);
  let name = $state('');
  let slug = $state('');
  let slugEdited = $state(false);
  let creating = $state(false);
  let createError = $state('');

  // Live-derive the slug from the name until the user edits the slug directly.
  $effect(() => {
    const derived = slugFromName(name);
    if (!slugEdited) slug = derived;
  });

  let slugInvalid = $derived(slug.length > 0 && !isValidSlug(slug));
  let canCreate = $derived(name.trim().length > 0 && isValidSlug(slug) && !creating);

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      memberships = await api.workspaces.list();
      // Seed the session cache the Nav switcher + breadcrumbs read from.
      setWorkspaces(memberships);
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  function openCreate() {
    showCreate = true;
    createError = '';
  }

  async function create(ev: SubmitEvent) {
    ev.preventDefault();
    if (!canCreate) return;
    creating = true;
    createError = '';
    try {
      const ws = await api.workspaces.create(name.trim(), slug);
      // Refresh the cached list so the Nav switcher shows the new workspace.
      void loadWorkspaces(true);
      // A fresh workspace starts on the fully-usable Free plan — go straight to
      // work. The PlanBanner there spells out the solo limits; the billing page
      // keeps its "?new=1" welcome for anyone who lands on it.
      await goto(`/w/${ws.slug}`);
    } catch (e) {
      if (e instanceof ApiError && e.code === 'slug_taken') {
        createError = 'That slug is already taken — pick another.';
      } else if (e instanceof ApiError && e.code === 'invalid_slug') {
        createError = 'Invalid slug. Use 3–40 lowercase letters, numbers, or hyphens.';
      } else if (e instanceof ApiError && e.code === 'email_unverified') {
        createError =
          'Verify your email address before creating a workspace — use the “Resend link” banner at the top of the page.';
      } else {
        createError = errorText(e);
      }
    } finally {
      creating = false;
    }
  }
</script>

<svelte:head><title>Workspaces · Stake Dev Tool Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-5xl px-6 py-10">
  <div class="mb-8 flex items-end justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight">Workspaces</h1>
      <p class="mt-1 text-sm text-muted">Games, math revisions and share links live inside a workspace.</p>
    </div>
    {#if !showCreate && (memberships.length > 0 || loading)}
      <Button onclick={openCreate}>New workspace</Button>
    {/if}
  </div>

  {#if showCreate}
    <Card class="fade-in mb-8 p-6">
      <form class="flex flex-col gap-4" onsubmit={create}>
        <div class="flex items-center justify-between">
          <h2 class="text-base font-semibold">Create a workspace</h2>
          <button
            type="button"
            class="text-sm text-muted transition hover:text-text"
            onclick={() => (showCreate = false)}>Cancel</button
          >
        </div>
        <div class="grid gap-4 sm:grid-cols-2">
          <Input id="ws-name" label="Name" bind:value={name} placeholder="Acme Studios" required />
          <Input
            id="ws-slug"
            label="Slug"
            bind:value={slug}
            oninput={() => (slugEdited = true)}
            mono
            placeholder="acme-studios"
            error={slugInvalid ? 'Use 3–40 chars: a–z, 0–9, hyphens (not at the ends).' : undefined}
            hint="Used in URLs — app.stake.cloud/w/{slug || 'your-slug'}"
          />
        </div>

        {#if createError}
          <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
            {createError}
          </p>
        {/if}

        <div class="flex items-center gap-3">
          <Button type="submit" loading={creating} disabled={!canCreate}>Create workspace</Button>
        </div>
      </form>
    </Card>
  {/if}

  {#if loading}
    <div class="grid gap-3 sm:grid-cols-2">
      <Card class="p-5"><Skeleton lines={2} /></Card>
      <Card class="p-5"><Skeleton lines={2} /></Card>
    </div>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={load}>Retry</Button>
    </Card>
  {:else if memberships.length === 0 && !showCreate}
    <EmptyState title="Create your first workspace">
      A workspace is where your team's games and math revisions live. You'll be its owner and can
      invite others in seconds.
      {#snippet cta()}
        <Button onclick={openCreate}>New workspace</Button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="grid gap-3 sm:grid-cols-2">
      {#each memberships as m (m.workspace.id || m.workspace.slug)}
        <a
          href={`/w/${m.workspace.slug}`}
          class="fade-in group rounded-lg border border-border bg-surface p-5 transition hover:border-border-strong"
        >
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <h3 class="truncate font-semibold tracking-tight group-hover:text-text">
                {m.workspace.name}
              </h3>
              <p class="mt-0.5 truncate font-mono-tab text-xs text-muted">{m.workspace.slug}</p>
            </div>
            <Badge tone={roleTone(m.role)}>{m.role}</Badge>
          </div>
        </a>
      {/each}
    </div>
  {/if}
</main>
