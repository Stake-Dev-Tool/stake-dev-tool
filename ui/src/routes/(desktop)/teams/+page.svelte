<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';

  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Badge } from '$lib/components/ui/badge';
  import { Toaster } from '$lib/components/ui/sonner';
  import { toast } from 'svelte-sonner';

  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import UsersIcon from '@lucide/svelte/icons/users';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import LogInIcon from '@lucide/svelte/icons/log-in';
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
  import SendIcon from '@lucide/svelte/icons/send';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import CheckIcon from '@lucide/svelte/icons/check';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import UploadCloudIcon from '@lucide/svelte/icons/upload-cloud';

  import {
    cloudApi,
    githubAuth,
    teamsApi,
    type CloudUser,
    type GithubUser,
    type LegacyTeam,
    type SyncReport,
    type Team,
    type TeamRole
  } from '$lib/api';
  import CloudSignInDialog from '$lib/components/CloudSignInDialog.svelte';
  import GithubSignInDialog from '$lib/components/GithubSignInDialog.svelte';

  let cloudUser = $state<CloudUser | null>(null);
  let loading = $state(true);
  let busy = $state(false);

  let workspaces = $state<Team[]>([]);
  let activeId = $state<string | null>(null);

  let cloudSignInOpen = $state(false);

  // Create workspace
  let createOpen = $state(false);
  let createName = $state('');
  let createSlug = $state('');

  // Join workspace (invite token)
  let joinOpen = $state(false);
  let joinToken = $state('');

  // Invite (create + one-time copy)
  let inviteOpen = $state(false);
  let inviteTargetId = $state<string | null>(null);
  let inviteRole = $state<TeamRole>('member');
  let inviteUrl = $state<string | null>(null);
  let inviteCopied = $state(false);

  // Delete workspace (owner)
  let deleteOpen = $state(false);
  let deleteTarget = $state<Team | null>(null);
  let deleteConfirm = $state('');

  // Legacy GitHub teams + migration
  let githubUser = $state<GithubUser | null>(null);
  let legacyTeams = $state<LegacyTeam[]>([]);
  let githubSignInOpen = $state(false);
  let migratingId = $state<string | null>(null);

  const activeWs = $derived(workspaces.find((w) => w.id === activeId) ?? null);
  const otherWs = $derived(workspaces.filter((w) => w.id !== activeId));
  const pendingLegacy = $derived(legacyTeams.filter((t) => !t.migratedTo));

  onMount(() => {
    (async () => {
      try {
        cloudUser = await cloudApi.currentUser();
        if (cloudUser) await refreshWorkspaces();
        await refreshLegacy();
      } catch (e) {
        console.error(e);
      } finally {
        loading = false;
      }
    })();
  });

  async function withBusy<T>(fn: () => Promise<T>): Promise<T | undefined> {
    busy = true;
    try {
      return await fn();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  async function refreshWorkspaces() {
    workspaces = await teamsApi.list();
    const active = await teamsApi.active();
    activeId = active?.id ?? null;
    if (activeId) cloudApi.subscribe(activeId).catch(() => {});
  }

  async function refreshLegacy() {
    try {
      githubUser = await githubAuth.currentUser();
      legacyTeams = await teamsApi.legacyList();
    } catch {
      legacyTeams = [];
    }
  }

  async function onCloudSignedIn(u: CloudUser) {
    cloudUser = u;
    await refreshWorkspaces();
  }

  async function signOutCloud() {
    if (!confirm('Sign out of the cloud? Workspaces stay on the server; syncing pauses here.'))
      return;
    await withBusy(async () => {
      await cloudApi.unsubscribe().catch(() => {});
      await cloudApi.signOut();
      cloudUser = null;
      workspaces = [];
      activeId = null;
      toast.success('Signed out');
    });
  }

  async function createWorkspace() {
    const name = createName.trim();
    if (!name) return toast.error('Give the workspace a name');
    await withBusy(async () => {
      const w = await teamsApi.create(name, createSlug.trim() || null);
      createOpen = false;
      createName = '';
      createSlug = '';
      await refreshWorkspaces();
      await setActive(w);
      toast.success(`Workspace "${w.name}" created`);
    });
  }

  async function joinWorkspace() {
    const token = joinToken.trim();
    if (!token) return toast.error('Paste an invite token or URL');
    // Accept a full invite URL by taking its last path segment.
    const cleaned = token.includes('/') ? token.split('/').filter(Boolean).pop()! : token;
    await withBusy(async () => {
      const w = await teamsApi.join(cleaned);
      joinOpen = false;
      joinToken = '';
      await refreshWorkspaces();
      await setActive(w);
      toast.success(`Joined "${w.name}"`);
    });
  }

  async function setActive(w: Team) {
    activeId = w.id;
    await teamsApi.setActive(w.id);
    cloudApi.subscribe(w.id).catch(() => {});
    toast.success(`"${w.name}" is now active`);
  }

  async function leaveWs(w: Team) {
    if (w.role === 'owner') {
      deleteTarget = w;
      deleteConfirm = '';
      deleteOpen = true;
      return;
    }
    if (!confirm(`Leave "${w.name}"? You'll lose access until you're re-invited.`)) return;
    await withBusy(async () => {
      await teamsApi.leave(w.id);
      await refreshWorkspaces();
      toast.success(`Left "${w.name}"`);
    });
  }

  async function confirmDelete() {
    const w = deleteTarget;
    if (!w) return;
    if (deleteConfirm.trim() !== w.name) return toast.error('Type the workspace name to confirm');
    await withBusy(async () => {
      await teamsApi.delete(w.id);
      deleteOpen = false;
      deleteTarget = null;
      deleteConfirm = '';
      await refreshWorkspaces();
      toast.success(`Workspace "${w.name}" deleted`);
    });
  }

  function openInvite(w: Team) {
    inviteTargetId = w.id;
    inviteRole = 'member';
    inviteUrl = null;
    inviteCopied = false;
    inviteOpen = true;
  }

  async function createInvite() {
    if (!inviteTargetId) return;
    await withBusy(async () => {
      inviteUrl = await teamsApi.invite(inviteTargetId!, inviteRole);
      inviteCopied = false;
      toast.success('Invite created — copy the link below (shown once)');
    });
  }

  async function copyInvite() {
    if (!inviteUrl) return;
    try {
      await navigator.clipboard.writeText(inviteUrl);
      inviteCopied = true;
      toast.success('Invite link copied');
    } catch {
      toast.error('Could not copy to clipboard');
    }
  }

  async function syncWs(w: Team) {
    await withBusy(async () => {
      const r: SyncReport = await teamsApi.sync(w.id);
      if (r.pushed + r.pulled === 0 && r.conflicts === 0) {
        toast.success('Already up to date');
      } else {
        const parts = [`↑${r.pushed}`, `↓${r.pulled}`];
        if (r.conflicts > 0) parts.push(`${r.conflicts} conflict${r.conflicts === 1 ? '' : 's'}`);
        toast.success(`Synced: ${parts.join(' · ')}`);
      }
    });
  }

  async function onGithubSignedIn(u: GithubUser) {
    githubUser = u;
    await refreshLegacy();
  }

  async function migrate(t: LegacyTeam) {
    if (!cloudUser) return toast.error('Sign in to the cloud first');
    if (!githubUser) return toast.error('Sign in to GitHub to read the team repo');
    if (
      !confirm(
        `Migrate "${t.name}" into a new cloud workspace? Profiles, saved rounds and math are copied. The GitHub repo is left untouched.`
      )
    )
      return;
    migratingId = t.id;
    try {
      const r = await teamsApi.migrateToCloud(t.id);
      toast.success(
        `Migrated "${t.name}" → "${r.workspaceName}" (${r.profiles} profiles, ${r.rounds} rounds, ${r.games} games)`
      );
      await refreshWorkspaces();
      await refreshLegacy();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      migratingId = null;
    }
  }
</script>

<svelte:head>
  <title>Teams · Stake Dev Tool</title>
</svelte:head>

<Toaster position="top-right" richColors closeButton />

<main class="mx-auto flex min-h-screen w-full max-w-4xl flex-col gap-8 px-8 py-10">
  <!-- Topbar -->
  <header class="flex items-center justify-between">
    <div class="flex items-center gap-4">
      <Button variant="ghost" size="icon-lg" onclick={() => goto('/')} aria-label="Back">
        <ArrowLeftIcon />
      </Button>
      <div>
        <h1 class="text-2xl font-semibold tracking-tight">Workspaces</h1>
        <p class="text-sm text-muted-foreground">
          Share profiles, saved rounds, and math across your team — live.
        </p>
      </div>
    </div>

    {#if cloudUser}
      <div class="flex flex-col items-end">
        <span class="text-sm font-medium">{cloudUser.display_name}</span>
        <button
          type="button"
          class="text-xs text-muted-foreground hover:text-foreground"
          onclick={signOutCloud}
          disabled={busy}
        >
          Sign out
        </button>
      </div>
    {/if}
  </header>

  {#if loading}
    <Card.Root>
      <Card.Content class="py-10 text-center text-sm text-muted-foreground">Loading…</Card.Content>
    </Card.Root>
  {:else if !cloudUser}
    <!-- Signed out -->
    <Card.Root>
      <Card.Header>
        <Card.Title class="flex items-center gap-2">
          <LogInIcon class="h-5 w-5" />
          Sign in to the cloud
        </Card.Title>
        <Card.Description>
          Workspaces live on the cloud platform. Profiles and saved rounds sync automatically and
          update live between members over a server event stream.
        </Card.Description>
      </Card.Header>
      <Card.Content>
        <Button size="lg" onclick={() => (cloudSignInOpen = true)} disabled={busy}>
          <LogInIcon />
          Sign in
        </Button>
      </Card.Content>
    </Card.Root>
  {:else}
    <!-- Active workspace -->
    {#if activeWs}
      <Card.Root class="border-emerald-500/30 bg-emerald-500/5">
        <Card.Header>
          <div class="flex items-start justify-between gap-4">
            <div>
              <Card.Title class="flex items-center gap-2">
                <UsersIcon class="h-5 w-5" />
                {activeWs.name}
                <Badge variant={activeWs.role === 'owner' ? 'secondary' : 'outline'}>
                  {activeWs.role}
                </Badge>
              </Card.Title>
              <Card.Description class="font-mono-tab mt-1">
                {activeWs.slug}
                {#if activeWs.memberCount != null}
                  · {activeWs.memberCount} member{activeWs.memberCount === 1 ? '' : 's'}
                {/if}
              </Card.Description>
            </div>
          </div>
        </Card.Header>
        <Card.Content class="flex flex-wrap items-center gap-2">
          <Button size="sm" onclick={() => syncWs(activeWs)} disabled={busy}>
            <RefreshIcon class={busy ? 'animate-spin' : ''} />
            Sync now
          </Button>
          {#if activeWs.role === 'owner' || activeWs.role === 'admin'}
            <Button size="sm" variant="outline" onclick={() => openInvite(activeWs)}>
              <SendIcon />
              Invite member
            </Button>
          {/if}
          <Button
            size="sm"
            variant="ghost"
            class="ml-auto text-destructive hover:text-destructive"
            onclick={() => leaveWs(activeWs)}
            disabled={busy}
          >
            <TrashIcon />
            {activeWs.role === 'owner' ? 'Delete workspace' : 'Leave'}
          </Button>
        </Card.Content>
      </Card.Root>
    {:else}
      <Card.Root>
        <Card.Content class="py-10 text-center">
          <p class="text-sm text-muted-foreground">You're not in any workspace yet.</p>
        </Card.Content>
      </Card.Root>
    {/if}

    <!-- Actions -->
    <div class="flex gap-2">
      <Button onclick={() => (createOpen = true)} disabled={busy}>
        <PlusIcon />
        Create workspace
      </Button>
      <Button variant="outline" onclick={() => (joinOpen = true)} disabled={busy}>
        <LogInIcon />
        Join with invite
      </Button>
    </div>

    <!-- Other workspaces -->
    {#if otherWs.length > 0}
      <div>
        <h2 class="mb-3 text-sm font-medium text-muted-foreground">Your other workspaces</h2>
        <div class="flex flex-col gap-2">
          {#each otherWs as w (w.id)}
            <Card.Root>
              <Card.Content class="flex items-center justify-between gap-3 py-4">
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="font-medium">{w.name}</span>
                    <Badge variant={w.role === 'owner' ? 'secondary' : 'outline'} class="text-xs">
                      {w.role}
                    </Badge>
                  </div>
                  <div class="font-mono-tab text-xs text-muted-foreground">
                    {w.slug}
                    {#if w.memberCount != null}· {w.memberCount}
                      member{w.memberCount === 1 ? '' : 's'}{/if}
                  </div>
                </div>
                <div class="flex gap-2">
                  <Button size="sm" variant="outline" onclick={() => setActive(w)} disabled={busy}>
                    <CheckIcon />
                    Set active
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    class="text-destructive hover:text-destructive"
                    onclick={() => leaveWs(w)}
                    disabled={busy}
                  >
                    <TrashIcon />
                  </Button>
                </div>
              </Card.Content>
            </Card.Root>
          {/each}
        </div>
      </div>
    {/if}
  {/if}

  <!-- Legacy GitHub teams (deprecated) -->
  {#if pendingLegacy.length > 0}
    <div class="mt-4 flex flex-col gap-3">
      <div
        class="rounded-md border border-amber-500/40 bg-amber-500/5 px-4 py-3 text-sm text-muted-foreground"
      >
        <span class="font-medium text-foreground">Legacy GitHub teams are deprecated.</span>
        Migrate each one into a cloud workspace to keep syncing. Migration copies profiles, saved
        rounds and math; the GitHub repo is left untouched. Requires both GitHub and cloud sign-in.
      </div>

      {#if !githubUser}
        <Button variant="outline" size="sm" class="self-start" onclick={() => (githubSignInOpen = true)}>
          <LogInIcon />
          Sign in with GitHub to migrate
        </Button>
      {/if}

      <div class="flex flex-col gap-2">
        {#each pendingLegacy as t (t.id)}
          <Card.Root class="border-dashed">
            <Card.Content class="flex items-center justify-between gap-3 py-4">
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span class="font-medium">{t.name}</span>
                  {#if t.role === 'owner'}
                    <Badge variant="secondary" class="text-xs">owner</Badge>
                  {/if}
                </div>
                <div class="font-mono-tab text-xs text-muted-foreground">
                  {t.repoOwner}/{t.repoName}
                </div>
              </div>
              <Button
                size="sm"
                onclick={() => migrate(t)}
                disabled={busy || migratingId === t.id || !cloudUser || !githubUser}
              >
                <UploadCloudIcon class={migratingId === t.id ? 'animate-pulse' : ''} />
                {migratingId === t.id ? 'Migrating…' : 'Migrate to cloud'}
              </Button>
            </Card.Content>
          </Card.Root>
        {/each}
      </div>
    </div>
  {/if}
</main>

<CloudSignInDialog bind:open={cloudSignInOpen} onSignedIn={onCloudSignedIn} />
<GithubSignInDialog bind:open={githubSignInOpen} onSignedIn={onGithubSignedIn} />

<!-- Create workspace dialog -->
<Dialog.Root bind:open={createOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Create a workspace</Dialog.Title>
      <Dialog.Description>You become the owner. Invite members with a link afterwards.</Dialog.Description>
    </Dialog.Header>
    <div class="my-4 flex flex-col gap-4">
      <div class="flex flex-col gap-2">
        <Label for="wsName">Name</Label>
        <Input id="wsName" bind:value={createName} placeholder="My Slot Team" />
      </div>
      <div class="flex flex-col gap-2">
        <Label for="wsSlug">Slug (optional)</Label>
        <Input id="wsSlug" bind:value={createSlug} placeholder="my-slot-team" />
        <p class="text-xs text-muted-foreground">Derived from the name if left blank.</p>
      </div>
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (createOpen = false)}>Cancel</Button>
      <Button onclick={createWorkspace} disabled={busy}>Create</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Join dialog -->
<Dialog.Root bind:open={joinOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Join a workspace</Dialog.Title>
      <Dialog.Description>Paste the invite link (or token) a workspace owner shared with you.</Dialog.Description>
    </Dialog.Header>
    <div class="my-4 flex flex-col gap-2">
      <Label for="joinToken">Invite link or token</Label>
      <Input id="joinToken" bind:value={joinToken} placeholder="https://…/invite/abc123 or abc123" />
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (joinOpen = false)}>Cancel</Button>
      <Button onclick={joinWorkspace} disabled={busy || !joinToken.trim()}>Join</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Invite dialog -->
<Dialog.Root bind:open={inviteOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Invite a member</Dialog.Title>
      <Dialog.Description>Create a one-time-view invite link and share it.</Dialog.Description>
    </Dialog.Header>
    <div class="my-4 flex flex-col gap-3">
      <div class="flex flex-col gap-2">
        <Label for="inviteRole">Role</Label>
        <select
          id="inviteRole"
          bind:value={inviteRole}
          class="h-9 rounded-md border border-input bg-background px-3 py-1 text-sm text-foreground shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <option value="member">member</option>
          <option value="admin">admin</option>
        </select>
      </div>
      {#if inviteUrl}
        <div class="flex flex-col gap-2">
          <Label>Invite link (copy now — shown once)</Label>
          <div class="flex items-center gap-2">
            <code
              class="font-mono-tab min-w-0 flex-1 truncate rounded-md border bg-muted px-3 py-2 text-xs"
              title={inviteUrl}>{inviteUrl}</code
            >
            <Button size="icon" variant={inviteCopied ? 'secondary' : 'outline'} onclick={copyInvite}>
              {#if inviteCopied}<CheckIcon />{:else}<CopyIcon />{/if}
            </Button>
          </div>
        </div>
      {/if}
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (inviteOpen = false)}>Close</Button>
      {#if !inviteUrl}
        <Button onclick={createInvite} disabled={busy}>
          <SendIcon />
          Create invite
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Delete workspace dialog -->
<Dialog.Root bind:open={deleteOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title class="text-destructive">Delete this workspace?</Dialog.Title>
      <Dialog.Description>
        This permanently deletes the workspace
        {#if deleteTarget}<span class="font-mono-tab text-foreground">{deleteTarget.slug}</span>{/if}
        and all its documents and math for every member. This cannot be undone.
      </Dialog.Description>
    </Dialog.Header>
    {#if deleteTarget}
      <div class="my-4 flex flex-col gap-2">
        <Label for="delConfirm">
          Type <span class="font-mono-tab text-foreground">{deleteTarget.name}</span> to confirm
        </Label>
        <Input id="delConfirm" bind:value={deleteConfirm} autocomplete="off" />
      </div>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (deleteOpen = false)}>Cancel</Button>
      <Button
        variant="destructive"
        onclick={confirmDelete}
        disabled={busy || !deleteTarget || deleteConfirm.trim() !== deleteTarget.name}
      >
        <TrashIcon />
        Delete workspace
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
