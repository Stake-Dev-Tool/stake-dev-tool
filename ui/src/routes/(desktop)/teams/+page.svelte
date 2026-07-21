<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { openUrl } from '@tauri-apps/plugin-opener';

  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Badge } from '$lib/components/ui/badge';
  import { Separator } from '$lib/components/ui/separator';
  import { Toaster } from '$lib/components/ui/sonner';
  import { toast } from 'svelte-sonner';

  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import UsersIcon from '@lucide/svelte/icons/users';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import LogInIcon from '@lucide/svelte/icons/log-in';
  import LogOutIcon from '@lucide/svelte/icons/log-out';
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
  import SendIcon from '@lucide/svelte/icons/send';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import CheckIcon from '@lucide/svelte/icons/check';

  import {
    githubAuth,
    teamsApi,
    type DiscoveredTeam,
    type GithubOrg,
    type GithubUser,
    type SyncReport,
    type Team
  } from '$lib/api';
  import GithubSignInDialog from '$lib/components/GithubSignInDialog.svelte';

  let user = $state<GithubUser | null>(null);
  let loading = $state(true);
  let busy = $state(false);

  let teams = $state<Team[]>([]);
  let activeTeamId = $state<string | null>(null);

  let signInOpen = $state(false);

  // Create team dialog
  let createOpen = $state(false);
  let createName = $state('');
  /// '' means "personal account" (no org).
  let createOrg = $state('');
  let orgs = $state<GithubOrg[]>([]);

  // Join team dialog
  let joinOpen = $state(false);
  let joinOwner = $state('');
  let joinRepo = $state('');
  let discovered = $state<DiscoveredTeam[]>([]);
  let discoverBusy = $state(false);

  // Invite dialog
  let inviteOpen = $state(false);
  let inviteUsername = $state('');
  let inviteTargetTeamId = $state<string | null>(null);

  // Delete team (owner) dialog
  let deleteOpen = $state(false);
  let deleteTarget = $state<Team | null>(null);
  let deleteConfirmText = $state('');


  const activeTeam = $derived(teams.find((t) => t.id === activeTeamId) ?? null);
  const otherTeams = $derived(teams.filter((t) => t.id !== activeTeamId));

  onMount(() => {
    (async () => {
      try {
        user = await githubAuth.currentUser();
        if (user) {
          await refreshTeams();
          refreshOrgs().catch(() => {});
        }
      } catch (e) {
        console.error(e);
      } finally {
        loading = false;
      }
    })();
  });

  async function refreshOrgs() {
    try {
      orgs = await githubAuth.listOrgs();
    } catch (e) {
      console.error(e);
      orgs = [];
    }
  }

  async function onSignedIn(u: GithubUser) {
    user = u;
    await refreshTeams();
    refreshOrgs().catch(() => {});
  }

  async function refreshTeams() {
    teams = await teamsApi.list();
    const active = await teamsApi.active();
    activeTeamId = active?.id ?? null;
  }

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

  async function signOut() {
    if (!confirm('Sign out of GitHub? Your local teams will remain but syncing will be disabled.'))
      return;
    await withBusy(async () => {
      await githubAuth.logout();
      user = null;
      toast.success('Signed out');
    });
  }

  // ---- Teams ----

  async function createTeam() {
    const name = createName.trim();
    if (!name) return toast.error('Give the team a name');
    const org = createOrg.trim() || null;
    await withBusy(async () => {
      const t = await teamsApi.create(name, org);
      createOpen = false;
      createName = '';
      createOrg = '';
      await refreshTeams();
      activeTeamId = t.id;
      await teamsApi.setActive(t.id);
      toast.success(`Team "${t.name}" created`);
    });
  }

  async function joinFromDiscovery(d: DiscoveredTeam) {
    await withBusy(async () => {
      const t = await teamsApi.join(d.repoOwner, d.repoName);
      joinOpen = false;
      await refreshTeams();
      activeTeamId = t.id;
      await teamsApi.setActive(t.id);
      toast.success(`Joined "${t.name}"`);
    });
  }

  async function joinManual() {
    const owner = joinOwner.trim();
    const repo = joinRepo.trim();
    if (!owner || !repo) return toast.error('Enter owner and repo name');
    await withBusy(async () => {
      const t = await teamsApi.join(owner, repo);
      joinOpen = false;
      joinOwner = '';
      joinRepo = '';
      await refreshTeams();
      activeTeamId = t.id;
      await teamsApi.setActive(t.id);
      toast.success(`Joined "${t.name}"`);
    });
  }

  async function openJoinDialog() {
    joinOpen = true;
    discoverBusy = true;
    discovered = [];
    try {
      const found = await teamsApi.discover();
      // Hide teams we already joined locally
      const localKeys = new Set(teams.map((t) => `${t.repoOwner}/${t.repoName}`));
      discovered = found.filter((d) => !localKeys.has(`${d.repoOwner}/${d.repoName}`));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      discoverBusy = false;
    }
  }

  async function setActive(t: Team) {
    activeTeamId = t.id;
    await teamsApi.setActive(t.id);
    toast.success(`"${t.name}" is now active`);
  }

  async function leaveTeam(t: Team) {
    if (t.role === 'owner') {
      deleteTarget = t;
      deleteConfirmText = '';
      deleteOpen = true;
      return;
    }
    if (
      !confirm(
        `Remove "${t.name}" from this device? Your local files stay. The GitHub repo is not deleted.`
      )
    )
      return;
    await withBusy(async () => {
      await teamsApi.leave(t.id);
      await refreshTeams();
      toast.success(`Left "${t.name}"`);
    });
  }

  async function confirmDeleteTeam() {
    const t = deleteTarget;
    if (!t) return;
    if (deleteConfirmText.trim() !== t.name) {
      return toast.error('Type the team name exactly to confirm');
    }
    await withBusy(async () => {
      await teamsApi.delete(t.id);
      deleteOpen = false;
      deleteTarget = null;
      deleteConfirmText = '';
      await refreshTeams();
      toast.success(`Team "${t.name}" deleted`);
    });
  }

  async function syncTeam(t: Team) {
    await withBusy(async () => {
      const r: SyncReport = await teamsApi.sync(t.id);
      await refreshTeams();
      const total =
        r.profilesPushed + r.profilesPulled + r.roundsPushed + r.roundsPulled;
      if (total === 0) {
        toast.success('Already up to date');
      } else {
        toast.success(
          `Synced: ↑${r.profilesPushed + r.roundsPushed} ↓${r.profilesPulled + r.roundsPulled}`
        );
      }
    });
  }

  function openInviteDialog(t: Team) {
    inviteTargetTeamId = t.id;
    inviteUsername = '';
    inviteOpen = true;
  }

  async function sendInvite() {
    const username = inviteUsername.trim();
    if (!inviteTargetTeamId || !username) return toast.error('Enter a GitHub username');
    await withBusy(async () => {
      await teamsApi.invite(inviteTargetTeamId!, username);
      inviteOpen = false;
      toast.success(`Invite sent to @${username}`);
    });
  }

  function formatRelative(ts: number | null | undefined): string {
    if (!ts) return 'never';
    const diff = Date.now() - ts;
    if (diff < 60_000) return 'just now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    const d = Math.floor(diff / 86_400_000);
    return d === 1 ? 'yesterday' : `${d}d ago`;
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
        <h1 class="text-2xl font-semibold tracking-tight">Teams</h1>
        <p class="text-sm text-muted-foreground">
          Share profiles, saved rounds, and math across your team
        </p>
      </div>
    </div>

    {#if user}
      <div class="flex items-center gap-3">
        {#if user.avatar_url}
          <img
            src={user.avatar_url}
            alt="@{user.login}"
            class="h-8 w-8 rounded-full border"
            referrerpolicy="no-referrer"
          />
        {/if}
        <div class="flex flex-col items-end">
          <span class="text-sm font-medium">@{user.login}</span>
          <button
            type="button"
            class="text-xs text-muted-foreground hover:text-foreground"
            onclick={signOut}
            disabled={busy}
          >
            Sign out
          </button>
        </div>
      </div>
    {/if}
  </header>

  {#if loading}
    <Card.Root>
      <Card.Content class="py-10 text-center text-sm text-muted-foreground">
        Loading…
      </Card.Content>
    </Card.Root>
  {:else if !user}
    <!-- Signed out state -->
    <Card.Root>
      <Card.Header>
        <Card.Title class="flex items-center gap-2">
          <LogInIcon class="h-5 w-5" />
          Sign in with GitHub
        </Card.Title>
        <Card.Description>
          Teams are backed by private GitHub repositories — one per team. Profiles and saved
          rounds sync automatically between members. You'll need a free GitHub account.
        </Card.Description>
      </Card.Header>
      <Card.Content>
        <Button size="lg" onclick={() => (signInOpen = true)} disabled={busy}>
          <LogInIcon />
          Sign in with GitHub
        </Button>
      </Card.Content>
    </Card.Root>
  {:else}
    <!-- Signed in state -->

    {#if activeTeam}
      <Card.Root class="border-emerald-500/30 bg-emerald-500/5">
        <Card.Header>
          <div class="flex items-start justify-between gap-4">
            <div>
              <Card.Title class="flex items-center gap-2">
                <UsersIcon class="h-5 w-5" />
                {activeTeam.name}
                {#if activeTeam.role === 'owner'}
                  <Badge variant="secondary">Owner</Badge>
                {:else}
                  <Badge variant="outline">Member</Badge>
                {/if}
              </Card.Title>
              <Card.Description class="font-mono-tab mt-1">
                {activeTeam.repoOwner}/{activeTeam.repoName}
              </Card.Description>
            </div>
            <div class="text-right text-xs text-muted-foreground">
              Last sync<br />
              <span class="font-mono-tab">{formatRelative(activeTeam.lastSyncAt)}</span>
            </div>
          </div>
        </Card.Header>
        <Card.Content class="flex flex-wrap items-center gap-2">
          <Button size="sm" onclick={() => syncTeam(activeTeam)} disabled={busy}>
            <RefreshIcon class={busy ? 'animate-spin' : ''} />
            Sync now
          </Button>
          {#if activeTeam.role === 'owner'}
            <Button size="sm" variant="outline" onclick={() => openInviteDialog(activeTeam)}>
              <SendIcon />
              Invite member
            </Button>
          {/if}
          <Button size="sm" variant="outline" onclick={() => openUrl(activeTeam.htmlUrl)}>
            <ExternalLinkIcon />
            Open on GitHub
          </Button>
          <Button
            size="sm"
            variant="ghost"
            class="ml-auto text-destructive hover:text-destructive"
            onclick={() => leaveTeam(activeTeam)}
          >
            <TrashIcon />
            {activeTeam.role === 'owner' ? 'Delete team' : 'Leave'}
          </Button>
        </Card.Content>
      </Card.Root>
    {:else}
      <Card.Root>
        <Card.Content class="py-10 text-center">
          <p class="text-sm text-muted-foreground">You're not in any team yet.</p>
        </Card.Content>
      </Card.Root>
    {/if}

    {#if activeTeam}
      <Card.Root class="border-dashed">
        <Card.Content class="py-4 text-sm text-muted-foreground">
          Shared games show up in the <a
            href="/"
            class="font-medium text-foreground underline-offset-4 hover:underline">Games page</a
          >. Hover a local profile there and click the upload icon to share it with this team
          (profile + math + saved rounds).
        </Card.Content>
      </Card.Root>
    {/if}

    <!-- Actions -->
    <div class="flex gap-2">
      <Button onclick={() => (createOpen = true)} disabled={busy}>
        <PlusIcon />
        Create team
      </Button>
      <Button variant="outline" onclick={openJoinDialog} disabled={busy}>
        <LogInIcon />
        Join existing team
      </Button>
    </div>

    <!-- Other teams -->
    {#if otherTeams.length > 0}
      <div>
        <h2 class="mb-3 text-sm font-medium text-muted-foreground">Other teams on this device</h2>
        <div class="flex flex-col gap-2">
          {#each otherTeams as t (t.id)}
            <Card.Root>
              <Card.Content class="flex items-center justify-between gap-3 py-4">
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="font-medium">{t.name}</span>
                    {#if t.role === 'owner'}
                      <Badge variant="secondary" class="text-xs">Owner</Badge>
                    {/if}
                  </div>
                  <div class="font-mono-tab text-xs text-muted-foreground">
                    {t.repoOwner}/{t.repoName}
                  </div>
                </div>
                <div class="flex gap-2">
                  <Button size="sm" variant="outline" onclick={() => setActive(t)} disabled={busy}>
                    <CheckIcon />
                    Set active
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    class="text-destructive hover:text-destructive"
                    onclick={() => leaveTeam(t)}
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
</main>

<GithubSignInDialog bind:open={signInOpen} {onSignedIn} />

<!-- Create team dialog -->
<Dialog.Root bind:open={createOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Create a new team</Dialog.Title>
      <Dialog.Description>
        A private GitHub repo will be created to host the workspace.
      </Dialog.Description>
    </Dialog.Header>
    <div class="my-4 flex flex-col gap-4">
      <div class="flex flex-col gap-2">
        <Label for="teamName">Team name</Label>
        <Input id="teamName" bind:value={createName} placeholder="My Slot Team" />
        <p class="text-xs text-muted-foreground">
          Repo will be named <span class="font-mono-tab">stake-dev-tool-team-&lt;slug&gt;</span>.
        </p>
      </div>

      <div class="flex flex-col gap-2">
        <Label for="teamOrg">Account</Label>
        <select
          id="teamOrg"
          bind:value={createOrg}
          class="h-9 rounded-md border border-input bg-background px-3 py-1 text-sm text-foreground shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <option value="" class="bg-background text-foreground">
            @{user?.login} (personal)
          </option>
          {#each orgs as o (o.id)}
            <option value={o.login} class="bg-background text-foreground">
              {o.login} (organization)
            </option>
          {/each}
        </select>
        {#if orgs.length === 0}
          <p class="text-xs text-muted-foreground">
            No organizations found for your account. You can only create under your personal
            account.
          </p>
        {/if}
      </div>
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (createOpen = false)}>Cancel</Button>
      <Button onclick={createTeam} disabled={busy}>Create</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Join team dialog -->
<Dialog.Root bind:open={joinOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>Join a team</Dialog.Title>
      <Dialog.Description>
        Teams you've been invited to show up here automatically.
      </Dialog.Description>
    </Dialog.Header>

    <div class="my-2 flex flex-col gap-4">
      <div>
        <h3 class="mb-2 text-sm font-medium">Discovered</h3>
        {#if discoverBusy}
          <p class="text-xs text-muted-foreground">Searching…</p>
        {:else if discovered.length === 0}
          <p class="text-xs text-muted-foreground">
            No pending invites found. Enter a repo manually below.
          </p>
        {:else}
          <div class="flex flex-col gap-2">
            {#each discovered as d (d.repoOwner + '/' + d.repoName)}
              <button
                type="button"
                class="flex items-center justify-between rounded-md border px-3 py-2 text-left hover:bg-accent"
                onclick={() => joinFromDiscovery(d)}
                disabled={busy}
              >
                <div class="min-w-0">
                  <div class="font-medium">{d.teamName}</div>
                  <div class="font-mono-tab text-xs text-muted-foreground">
                    {d.repoOwner}/{d.repoName}
                  </div>
                </div>
                <CheckIcon class="h-4 w-4 text-muted-foreground" />
              </button>
            {/each}
          </div>
        {/if}
      </div>

      <Separator />

      <div>
        <h3 class="mb-2 text-sm font-medium">Or join manually</h3>
        <div class="flex flex-col gap-2">
          <Label for="joinOwner">Owner</Label>
          <Input id="joinOwner" bind:value={joinOwner} placeholder="alice" />
          <Label for="joinRepo">Repo name</Label>
          <Input id="joinRepo" bind:value={joinRepo} placeholder="stake-dev-tool-team-foo" />
        </div>
      </div>
    </div>

    <Dialog.Footer>
      <Button variant="outline" onclick={() => (joinOpen = false)}>Close</Button>
      <Button onclick={joinManual} disabled={busy || !joinOwner.trim() || !joinRepo.trim()}>
        Join manually
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Delete team (owner) dialog -->
<Dialog.Root bind:open={deleteOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title class="text-destructive">Delete this team?</Dialog.Title>
      <Dialog.Description>
        This will permanently delete the private GitHub repository
        {#if deleteTarget}
          <span class="font-mono-tab text-foreground"
            >{deleteTarget.repoOwner}/{deleteTarget.repoName}</span
          >
        {/if}
        and all math files, saved rounds and profiles stored in it. All
        members will lose access immediately. This cannot be undone.
      </Dialog.Description>
    </Dialog.Header>
    {#if deleteTarget}
      <div class="my-4 flex flex-col gap-2">
        <Label for="deleteConfirm">
          Type <span class="font-mono-tab text-foreground">{deleteTarget.name}</span> to confirm
        </Label>
        <Input id="deleteConfirm" bind:value={deleteConfirmText} autocomplete="off" />
      </div>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (deleteOpen = false)}>Cancel</Button>
      <Button
        variant="destructive"
        onclick={confirmDeleteTeam}
        disabled={busy || !deleteTarget || deleteConfirmText.trim() !== deleteTarget.name}
      >
        <TrashIcon />
        Delete team
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Invite dialog -->
<Dialog.Root bind:open={inviteOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Invite a team member</Dialog.Title>
      <Dialog.Description>
        GitHub will email them an invitation to collaborate on the private repo.
      </Dialog.Description>
    </Dialog.Header>
    <div class="my-4 flex flex-col gap-2">
      <Label for="inviteUser">GitHub username</Label>
      <Input id="inviteUser" bind:value={inviteUsername} placeholder="alice" />
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (inviteOpen = false)}>Cancel</Button>
      <Button onclick={sendInvite} disabled={busy || !inviteUsername.trim()}>
        <SendIcon />
        Send invite
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
