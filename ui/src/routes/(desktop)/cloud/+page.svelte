<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { openUrl } from '@tauri-apps/plugin-opener';

  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import { Badge } from '$lib/components/ui/badge';
  import { Toaster } from '$lib/components/ui/sonner';
  import { toast } from 'svelte-sonner';

  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import CloudIcon from '@lucide/svelte/icons/cloud';
  import Gamepad2Icon from '@lucide/svelte/icons/gamepad-2';
  import LayersIcon from '@lucide/svelte/icons/layers';
  import LogInIcon from '@lucide/svelte/icons/log-in';
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
  import DownloadCloudIcon from '@lucide/svelte/icons/download-cloud';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import GitCommitIcon from '@lucide/svelte/icons/git-commit-horizontal';

  import {
    cloudApi,
    teamsApi,
    onWorkspaceEvent,
    type CloudUser,
    type Team,
    type CloudGame,
    type CloudRevisionSummary,
    type CloudRevisionDetail,
    type CloudStatsStatus
  } from '$lib/api';
  import CloudSignInDialog from '$lib/components/CloudSignInDialog.svelte';

  let cloudUser = $state<CloudUser | null>(null);
  let loading = $state(true);
  let busy = $state(false);
  let cloudSignInOpen = $state(false);

  let workspaces = $state<Team[]>([]);
  let activeId = $state<string | null>(null);
  let baseUrl = $state('');

  const activeWs = $derived(workspaces.find((w) => w.id === activeId) ?? null);

  // Games / revisions browse state.
  let games = $state<CloudGame[]>([]);
  let gamesLoading = $state(false);
  let selectedGame = $state<string | null>(null);
  let revisions = $state<CloudRevisionSummary[]>([]);
  let revisionsLoading = $state(false);

  // Per-row expand → stats strip (cached by revision number).
  let expandedRev = $state<number | null>(null);
  let detailCache = $state<Record<number, CloudRevisionDetail>>({});
  let detailLoading = $state<number | null>(null);

  // Pull-in-flight guard (progress shown by the shared MathSyncOverlay).
  let pullingRev = $state<number | null>(null);

  // SSE "rev pushed — refresh" hint for the game currently open.
  let pushHint = $state<{ game: string; number: number } | null>(null);

  const selectedGameMeta = $derived(games.find((g) => g.slug === selectedGame) ?? null);

  onMount(() => {
    let unlisten: (() => void) | null = null;
    (async () => {
      try {
        cloudUser = await cloudApi.currentUser();
        baseUrl = await cloudApi.getBaseUrl().catch(() => '');
        if (cloudUser) await refreshWorkspaces();
      } catch (e) {
        console.error(e);
      } finally {
        loading = false;
      }

      // Live stream: surface a manual-refresh hint on revision_pushed. We never
      // auto-refresh the table (the user might be mid-read); just nudge.
      onWorkspaceEvent((ev) => {
        if (ev.type !== 'revision_pushed') return;
        if (selectedGame && ev.game === selectedGame) {
          pushHint = { game: ev.game, number: ev.number };
        }
        toast.info(`${ev.game}: rev ${ev.number} pushed — refresh to see it`);
      })
        .then((un) => {
          unlisten = un;
        })
        .catch(() => {});
    })();

    return () => unlisten?.();
  });

  async function onCloudSignedIn(u: CloudUser) {
    cloudUser = u;
    await refreshWorkspaces();
  }

  async function refreshWorkspaces() {
    workspaces = await teamsApi.list();
    const active = await teamsApi.active();
    activeId = active?.id ?? workspaces[0]?.id ?? null;
    if (activeId) {
      cloudApi.subscribe(activeId).catch(() => {});
      await loadGames();
    }
  }

  async function selectWorkspace(id: string) {
    if (id === activeId) return;
    activeId = id;
    selectedGame = null;
    revisions = [];
    await teamsApi.setActive(id);
    cloudApi.subscribe(id).catch(() => {});
    await loadGames();
  }

  async function loadGames() {
    if (!activeId) return;
    gamesLoading = true;
    try {
      games = await cloudApi.listGames(activeId);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
      games = [];
    } finally {
      gamesLoading = false;
    }
  }

  async function openGame(slug: string) {
    selectedGame = slug;
    pushHint = null;
    expandedRev = null;
    await loadRevisions();
  }

  function backToGames() {
    selectedGame = null;
    revisions = [];
    pushHint = null;
  }

  async function loadRevisions() {
    if (!activeId || !selectedGame) return;
    revisionsLoading = true;
    pushHint = null;
    try {
      revisions = await cloudApi.listRevisions(activeId, selectedGame);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
      revisions = [];
    } finally {
      revisionsLoading = false;
    }
  }

  async function toggleExpand(n: number) {
    if (expandedRev === n) {
      expandedRev = null;
      return;
    }
    expandedRev = n;
    if (!activeId || !selectedGame || detailCache[n]) return;
    detailLoading = n;
    try {
      const d = await cloudApi.revisionDetail(activeId, selectedGame, n);
      detailCache = { ...detailCache, [n]: d };
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
      expandedRev = null;
    } finally {
      detailLoading = null;
    }
  }

  async function pullRevision(n: number) {
    if (!activeId || !selectedGame) return;
    pullingRev = n;
    busy = true;
    try {
      toast.info(`Pulling ${selectedGame} rev ${n}… large games can take a while.`);
      await cloudApi.pullRevisionToProfile(activeId, selectedGame, n);
      toast.success(`Pulled rev ${n} — the profile is ready on the launcher`, {
        action: { label: 'Open launcher', onClick: () => goto('/') }
      });
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      pullingRev = null;
      busy = false;
    }
  }

  async function openInDashboard(n: number) {
    const slug = activeWs?.slug;
    if (!slug || !selectedGame) return;
    if (!baseUrl) {
      toast.error('Cloud base URL is not configured.');
      return;
    }
    const url = `${baseUrl}/w/${slug}/g/${selectedGame}/r/${n}`;
    try {
      await openUrl(url);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }

  function fmtBytes(n: number): string {
    if (!Number.isFinite(n) || n <= 0) return '0 B';
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  function fmtAge(iso: string): string {
    const t = Date.parse(iso);
    if (!Number.isFinite(t)) return '—';
    const diff = Date.now() - t;
    if (diff < 60_000) return 'just now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    const d = Math.floor(diff / 86_400_000);
    return d === 1 ? 'yesterday' : `${d}d ago`;
  }

  type StatsBadge = { label: string; class: string; pulse: boolean };
  function statsBadge(s: CloudStatsStatus | null | undefined): StatsBadge | null {
    if (s === 'pending') return { label: 'computing', class: 'text-blue-500 border-blue-500/50', pulse: true };
    if (s === 'ok') return { label: 'stats ok', class: 'text-emerald-500 border-emerald-500/50', pulse: false };
    if (s === 'error') return { label: 'stats error', class: 'text-destructive border-destructive/50', pulse: false };
    return null;
  }
</script>

<svelte:head>
  <title>Cloud · Stake Dev Tool</title>
</svelte:head>

<Toaster position="top-right" richColors closeButton />

<main class="mx-auto flex min-h-screen w-full max-w-4xl flex-col gap-8 px-8 py-10">
  <!-- Topbar -->
  <header class="flex items-center justify-between">
    <div class="flex items-center gap-4">
      <Button variant="ghost" size="icon-lg" onclick={() => goto('/')} aria-label="Back">
        <ArrowLeftIcon />
      </Button>
      <img src="/icon-128.png" alt="Stake Dev Tool" class="h-9 w-9 rounded-lg border" />
      <div>
        <h1 class="display flex items-center gap-2 text-2xl font-bold tracking-tight">
          <CloudIcon class="h-6 w-6 text-emerald-500" />
          Cloud
        </h1>
        <p class="text-sm text-muted-foreground">
          Browse workspace games and revisions — pull any revision straight into a launch profile.
        </p>
      </div>
    </div>

    {#if cloudUser}
      <div class="flex flex-col items-end">
        <span class="text-sm font-medium">{cloudUser.display_name}</span>
        <button
          type="button"
          class="text-xs text-muted-foreground hover:text-foreground"
          onclick={() => goto('/teams')}
        >
          Manage workspaces
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
          The cloud browser lists every game and math revision in your workspaces. Sign in to browse
          them and pull revisions into local launch profiles.
        </Card.Description>
      </Card.Header>
      <Card.Content>
        <Button size="lg" onclick={() => (cloudSignInOpen = true)} disabled={busy}>
          <LogInIcon />
          Sign in
        </Button>
      </Card.Content>
    </Card.Root>
  {:else if workspaces.length === 0}
    <Card.Root>
      <Card.Content class="py-10 text-center">
        <p class="text-sm text-muted-foreground">
          You're not in any workspace yet.
          <button class="underline hover:text-foreground" onclick={() => goto('/teams')}>
            Create or join one
          </button>
          to get started.
        </p>
      </Card.Content>
    </Card.Root>
  {:else}
    <!-- Workspace selector -->
    <div class="flex flex-wrap items-center gap-3">
      <label for="wsPick" class="text-sm font-medium text-muted-foreground">Workspace</label>
      <select
        id="wsPick"
        value={activeId}
        onchange={(e) => selectWorkspace((e.currentTarget as HTMLSelectElement).value)}
        class="h-9 min-w-[16rem] rounded-md border border-input bg-background px-3 py-1 text-sm text-foreground shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      >
        {#each workspaces as w (w.id)}
          <option value={w.id}>{w.name} ({w.slug})</option>
        {/each}
      </select>
      {#if activeWs}
        <Badge variant="outline" class="text-xs">{activeWs.role}</Badge>
      {/if}
      <Button
        variant="ghost"
        size="sm"
        class="ml-auto"
        onclick={selectedGame ? loadRevisions : loadGames}
        disabled={busy || gamesLoading || revisionsLoading}
        title="Refresh"
      >
        <RefreshIcon class={gamesLoading || revisionsLoading ? 'animate-spin' : ''} />
        Refresh
      </Button>
    </div>

    {#if !selectedGame}
      <!-- Games grid -->
      <section class="flex flex-col gap-4">
        <div class="flex items-center gap-2 border-b pb-2">
          <Gamepad2Icon class="h-4 w-4 text-muted-foreground" />
          <h2 class="text-sm font-semibold tracking-tight">Games</h2>
          <span class="text-xs text-muted-foreground">· {games.length}</span>
        </div>

        {#if gamesLoading}
          <Card.Root>
            <Card.Content class="py-10 text-center text-sm text-muted-foreground">Loading games…</Card.Content>
          </Card.Root>
        {:else if games.length === 0}
          <Card.Root class="border-dashed">
            <Card.Content class="py-10 text-center text-sm text-muted-foreground">
              This workspace has no games yet. Push math from the launcher or the CLI to create one.
            </Card.Content>
          </Card.Root>
        {:else}
          <div class="grid gap-3 sm:grid-cols-2">
            {#each games as g (g.slug)}
              <button
                type="button"
                class="group flex items-center gap-4 rounded-lg border bg-card p-4 text-left transition hover:border-primary/30"
                onclick={() => openGame(g.slug)}
              >
                <div class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg border bg-muted text-muted-foreground">
                  <Gamepad2Icon class="h-5 w-5" />
                </div>
                <div class="min-w-0 flex-1">
                  <div class="truncate font-semibold tracking-tight">{g.name || g.slug}</div>
                  <div class="font-mono-tab truncate text-xs text-muted-foreground">{g.slug}</div>
                </div>
                <div class="flex flex-col items-end gap-1">
                  {#if g.head_number != null}
                    <Badge variant="secondary" class="text-xs">rev {g.head_number}</Badge>
                  {:else}
                    <Badge variant="outline" class="text-xs">no revisions</Badge>
                  {/if}
                  {#if g.revisions_count != null && g.revisions_count > 0}
                    <span class="font-mono-tab text-[10px] text-muted-foreground">
                      {g.revisions_count} total
                    </span>
                  {/if}
                </div>
                <ChevronRightIcon class="h-4 w-4 text-muted-foreground transition group-hover:translate-x-0.5" />
              </button>
            {/each}
          </div>
        {/if}
      </section>
    {:else}
      <!-- Revisions view -->
      <section class="flex flex-col gap-4">
        <div class="flex items-center gap-2 border-b pb-2">
          <Button variant="ghost" size="icon" onclick={backToGames} aria-label="Back to games">
            <ArrowLeftIcon class="h-4 w-4" />
          </Button>
          <LayersIcon class="h-4 w-4 text-muted-foreground" />
          <h2 class="text-sm font-semibold tracking-tight">
            {selectedGameMeta?.name || selectedGame}
          </h2>
          <Badge variant="secondary" class="font-mono-tab text-xs">{selectedGame}</Badge>
          <span class="text-xs text-muted-foreground">· {revisions.length} revisions</span>
        </div>

        {#if pushHint}
          <div class="flex items-center justify-between gap-3 rounded-md border border-blue-500/40 bg-blue-500/5 px-4 py-2.5 text-sm">
            <span class="text-muted-foreground">
              <span class="font-medium text-foreground">rev {pushHint.number} pushed.</span>
              Refresh to see it.
            </span>
            <Button size="sm" variant="outline" onclick={loadRevisions} disabled={revisionsLoading}>
              <RefreshIcon class={revisionsLoading ? 'animate-spin' : ''} />
              Refresh
            </Button>
          </div>
        {/if}

        {#if revisionsLoading}
          <Card.Root>
            <Card.Content class="py-10 text-center text-sm text-muted-foreground">Loading revisions…</Card.Content>
          </Card.Root>
        {:else if revisions.length === 0}
          <Card.Root class="border-dashed">
            <Card.Content class="py-10 text-center text-sm text-muted-foreground">
              No revisions yet for this game.
            </Card.Content>
          </Card.Root>
        {:else}
          <Card.Root class="overflow-hidden">
            <div class="overflow-x-auto">
              <table class="w-full min-w-[52rem] text-sm">
                <thead>
                  <tr class="border-b text-left text-xs uppercase tracking-wide text-muted-foreground">
                    <th class="w-8 px-2 py-3"></th>
                    <th class="px-3 py-3 font-medium">Rev</th>
                    <th class="px-3 py-3 font-medium">Message</th>
                    <th class="px-3 py-3 font-medium">Author</th>
                    <th class="px-3 py-3 font-medium">Age</th>
                    <th class="px-3 py-3 text-right font-medium">Files</th>
                    <th class="px-3 py-3 text-right font-medium">Size</th>
                    <th class="px-3 py-3 font-medium">Stats</th>
                    <th class="px-3 py-3 text-right font-medium">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {#each revisions as r (r.number)}
                    {@const badge = statsBadge(r.stats_status)}
                    {@const expanded = expandedRev === r.number}
                    <tr class="border-b border-border/60 transition last:border-0 hover:bg-muted/40">
                      <td class="px-2 py-3">
                        <button
                          type="button"
                          class="flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                          onclick={() => toggleExpand(r.number)}
                          aria-label={expanded ? 'Collapse stats' : 'Expand stats'}
                        >
                          {#if expanded}
                            <ChevronDownIcon class="h-4 w-4" />
                          {:else}
                            <ChevronRightIcon class="h-4 w-4" />
                          {/if}
                        </button>
                      </td>
                      <td class="px-3 py-3 font-mono-tab font-semibold">{r.number}</td>
                      <td class="px-3 py-3">
                        <span class="line-clamp-1 max-w-[18rem]">{r.message || '—'}</span>
                      </td>
                      <td class="px-3 py-3 text-muted-foreground">{r.author_display_name || '—'}</td>
                      <td class="px-3 py-3 text-muted-foreground">{fmtAge(r.created_at)}</td>
                      <td class="px-3 py-3 text-right font-mono-tab text-muted-foreground">{r.files_count}</td>
                      <td class="px-3 py-3 text-right font-mono-tab text-muted-foreground">{fmtBytes(r.total_size)}</td>
                      <td class="px-3 py-3">
                        {#if badge}
                          <Badge variant="outline" class="{badge.class} {badge.pulse ? 'animate-pulse' : ''} text-xs">
                            {badge.label}
                          </Badge>
                        {:else}
                          <span class="text-muted-foreground">—</span>
                        {/if}
                      </td>
                      <td class="px-3 py-3">
                        <div class="flex items-center justify-end gap-1">
                          <Button
                            size="sm"
                            onclick={() => pullRevision(r.number)}
                            disabled={busy}
                            title="Pull this revision into a launch profile"
                          >
                            <DownloadCloudIcon class={pullingRev === r.number ? 'animate-pulse' : ''} />
                            {pullingRev === r.number ? 'Pulling…' : 'Pull'}
                          </Button>
                          <Button
                            size="icon"
                            variant="ghost"
                            onclick={() => openInDashboard(r.number)}
                            title="Open in dashboard"
                            aria-label="Open in dashboard"
                          >
                            <ExternalLinkIcon />
                          </Button>
                        </div>
                      </td>
                    </tr>
                    {#if expanded}
                      <tr class="border-b border-border/60 bg-muted/20 last:border-0">
                        <td></td>
                        <td colspan="8" class="px-3 py-3">
                          {#if detailLoading === r.number}
                            <span class="text-xs text-muted-foreground">Loading stats…</span>
                          {:else if detailCache[r.number]}
                            {@const d = detailCache[r.number]}
                            {#if d.stats && d.stats.modes.length > 0}
                              <div class="flex flex-wrap gap-2">
                                {#each d.stats.modes as m (m.mode)}
                                  <div class="flex items-center gap-3 rounded-md border bg-card px-3 py-2">
                                    <GitCommitIcon class="h-3.5 w-3.5 text-muted-foreground" />
                                    <span class="font-mono-tab text-xs font-medium">{m.mode}</span>
                                    <span class="text-xs text-muted-foreground">
                                      RTP <span class="font-mono-tab text-foreground">{(m.rtp * 100).toFixed(2)}%</span>
                                    </span>
                                    <span class="text-xs text-muted-foreground">
                                      max win <span class="font-mono-tab text-foreground">×{m.max_win.toFixed(0)}</span>
                                    </span>
                                    {#if m.cost}
                                      <span class="text-xs text-muted-foreground">
                                        cost <span class="font-mono-tab text-foreground">{m.cost}×</span>
                                      </span>
                                    {/if}
                                  </div>
                                {/each}
                              </div>
                            {:else if d.stats && d.stats.status === 'error'}
                              <span class="text-xs text-destructive">
                                Stats failed{d.stats.error ? `: ${d.stats.error}` : ''}
                              </span>
                            {:else if d.stats && d.stats.status === 'pending'}
                              <span class="text-xs text-muted-foreground">Stats still computing…</span>
                            {:else}
                              <span class="text-xs text-muted-foreground">No stats for this revision.</span>
                            {/if}
                          {/if}
                        </td>
                      </tr>
                    {/if}
                  {/each}
                </tbody>
              </table>
            </div>
          </Card.Root>
        {/if}
      </section>
    {/if}
  {/if}
</main>

<CloudSignInDialog bind:open={cloudSignInOpen} onSignedIn={onCloudSignedIn} />
