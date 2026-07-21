<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { api, ApiError, isUnauthorized, type InvitePreview } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { roleTone, errorText } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';

  let token = $derived(page.params.token ?? '');

  let preview = $state<InvitePreview | null>(null);
  let loading = $state(true);
  let loadError = $state('');
  let accepting = $state(false);
  let acceptError = $state('');

  let loginHref = $derived(`/login?next=${encodeURIComponent(`/invite/${token}`)}`);

  onMount(async () => {
    try {
      preview = await api.invites.preview(token);
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  });

  async function accept() {
    if (accepting) return;
    // Not signed in → send to login and come back here.
    if (!session.user) {
      await goto(loginHref);
      return;
    }
    accepting = true;
    acceptError = '';
    try {
      const membership = await api.invites.accept(token);
      // The accept response may not carry the slug; fall back to matching the
      // freshly-joined workspace by name from the workspace list.
      let slug = membership.workspace.slug;
      if (!slug && preview) {
        const list = await api.workspaces.list();
        slug = list.find((m) => m.workspace.name === preview!.workspace_name)?.workspace.slug ?? '';
      }
      await goto(slug ? `/w/${slug}` : '/');
    } catch (e) {
      if (isUnauthorized(e)) {
        await goto(loginHref);
        return;
      }
      if (e instanceof ApiError && e.code === 'already_member' && preview) {
        // Idempotent: already in — just go to the workspaces list.
        await goto('/');
        return;
      }
      if (e instanceof ApiError && e.code === 'upgrade_required') {
        // The workspace is at its plan's member cap (or its trial lapsed). The
        // invitee can't act on billing — point them at the owner rather than a
        // billing page they have no access to.
        const name = preview?.workspace_name ? `"${preview.workspace_name}"` : 'This workspace';
        acceptError = `${name} can't add members on its current plan. Ask the workspace owner to upgrade, then open your invite again.`;
        return;
      }
      acceptError = errorText(e);
    } finally {
      accepting = false;
    }
  }
</script>

<svelte:head><title>Invite · Stake Cloud</title></svelte:head>

<main class="flex min-h-screen items-center justify-center px-6 py-12">
  <div class="fade-in w-full max-w-md">
    <div class="mb-8 flex justify-center">
      <a href="/" class="flex items-center gap-2.5">
        <span
          class="flex h-8 w-8 items-center justify-center rounded-lg bg-accent text-sm font-bold text-accent-ink"
          >S</span
        >
        <span class="font-semibold tracking-tight">Stake Cloud</span>
      </a>
    </div>

    <Card class="p-8 text-center">
      {#if loading}
        <div class="flex items-center justify-center gap-3 py-8 text-muted">
          <span class="spinner"></span> Checking invite…
        </div>
      {:else if loadError || !preview || !preview.valid}
        <div class="flex flex-col items-center gap-3">
          <span class="flex h-11 w-11 items-center justify-center rounded-full bg-danger/10 text-danger text-xl">!</span>
          <h1 class="text-lg font-semibold">This invite isn't valid</h1>
          <p class="text-sm text-muted">
            It may have expired, been revoked, or already been used up. Ask a workspace admin for a
            fresh link.
          </p>
          <Button href="/" variant="outline" class="mt-2">Go to Stake Cloud</Button>
        </div>
      {:else}
        <h1 class="text-lg font-semibold">You've been invited</h1>
        <p class="mt-2 text-sm text-muted">
          {#if preview.inviter_display_name}<span class="text-text">{preview.inviter_display_name}</span>
            invited you to join{:else}You've been invited to join{/if}
        </p>
        <div class="my-5 rounded-lg border border-border bg-surface-2 px-4 py-4">
          <div class="text-lg font-semibold tracking-tight">{preview.workspace_name}</div>
          <div class="mt-1.5 flex items-center justify-center gap-2 text-sm text-muted">
            as <Badge tone={roleTone(preview.role)}>{preview.role}</Badge>
          </div>
        </div>

        {#if acceptError}
          <p class="mb-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
            {acceptError}
          </p>
        {/if}

        {#if session.user}
          <Button class="w-full" loading={accepting} onclick={accept}>Accept invite</Button>
          <p class="mt-3 text-xs text-faint">
            Joining as {session.user.display_name || session.user.email}
          </p>
        {:else}
          <Button class="w-full" href={loginHref}>Sign in to accept</Button>
          <p class="mt-3 text-xs text-faint">You'll come right back here after signing in.</p>
        {/if}
      {/if}
    </Card>
  </div>
</main>
