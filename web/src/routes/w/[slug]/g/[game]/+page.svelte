<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { api, ApiError, type Game, type RevisionSummary, type StatsStatus } from '$lib/api';
  import { errorText, relativeAge, humanSize } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import MathPushPanel from '$lib/components/MathPushPanel.svelte';
  import FrontUrlDialog from '$lib/components/FrontUrlDialog.svelte';
  import PlanBanner from '$lib/components/PlanBanner.svelte';
  import SharePanel from '$lib/components/SharePanel.svelte';

  let slug = $derived(page.params.slug ?? '');
  let game = $derived(page.params.game ?? '');

  let showPush = $state(false);
  let testOpen = $state(false);

  let gameMeta = $state<Game | null>(null);
  let revisions = $state<RevisionSummary[]>([]);
  let loading = $state(true);
  let loadError = $state('');
  let notFound = $state(false);

  // Compare picker (after / before revision numbers).
  let cmpAfter = $state<number | null>(null);
  let cmpBefore = $state<number | null>(null);

  let headNumber = $derived(gameMeta?.head_number ?? revisions[0]?.number ?? null);
  let gameName = $derived(gameMeta?.name || game);
  let canCompare = $derived(cmpAfter != null && cmpBefore != null && cmpAfter !== cmpBefore);

  // SvelteKit reuses this component across /w/:slug/g/* navigations, so track the
  // params and reload when they change.
  $effect(() => {
    void slug;
    void game;
    load();
  });

  async function load() {
    loading = true;
    loadError = '';
    notFound = false;
    try {
      const [list, revs] = await Promise.all([
        api.games.list(slug),
        api.games.revisions(slug, game)
      ]);
      revisions = revs;
      gameMeta = list.find((g) => g.slug === game) ?? null;
      // Default the compare picker to "newest vs the one before it".
      cmpAfter = revs[0]?.number ?? null;
      cmpBefore = (revs.length >= 2 ? revs[1].number : revs[0]?.number) ?? null;
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) notFound = true;
      else loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  function openRevision(n: number) {
    void goto(`/w/${slug}/g/${game}/r/${n}`);
  }

  function onPushed(n: number) {
    showPush = false;
    // Jump to the new revision (its stats poll there); fall back to a reload if
    // the commit response carried no usable number.
    if (n >= 1) void goto(`/w/${slug}/g/${game}/r/${n}`);
    else void load();
  }

  function compare() {
    if (!canCompare) return;
    void goto(`/w/${slug}/g/${game}/diff/${cmpAfter}/${cmpBefore}`);
  }

  type Tone = 'neutral' | 'accent' | 'danger';
  function statsBadge(s: StatsStatus | null): { label: string; tone: Tone; pulse: boolean } | null {
    if (s === 'pending') return { label: 'computing', tone: 'neutral', pulse: true };
    if (s === 'ok') return { label: 'stats ok', tone: 'accent', pulse: false };
    if (s === 'error') return { label: 'stats error', tone: 'danger', pulse: false };
    return null;
  }
</script>

<svelte:head><title>{gameName} · Stake Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-5xl px-6 py-10">
  <a
    href={`/w/${slug}`}
    class="mb-6 inline-flex items-center gap-1.5 text-sm text-muted transition hover:text-text"
  >
    <span aria-hidden="true">←</span> Workspace
  </a>

  {#if loading}
    <div class="flex items-center gap-3 py-16 text-muted"><span class="spinner"></span> Loading…</div>
  {:else if notFound}
    <Card class="flex flex-col items-center gap-3 border-dashed px-6 py-16 text-center">
      <span class="flex h-11 w-11 items-center justify-center rounded-full bg-surface-2 text-xl text-muted">?</span>
      <h1 class="text-lg font-semibold">Game not found</h1>
      <p class="max-w-sm text-sm text-muted">
        This game doesn't exist in the workspace, or you don't have access to it.
      </p>
      <Button href={`/w/${slug}`} variant="outline" class="mt-2">Back to workspace</Button>
    </Card>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={load}>Retry</Button>
    </Card>
  {:else}
    <header class="mb-8 flex flex-wrap items-center gap-3">
      <h1 class="text-2xl font-semibold tracking-tight">{gameName}</h1>
      <span class="font-mono-tab text-sm text-muted">{game}</span>
      {#if headNumber != null}
        <Badge tone="accent">rev {headNumber}</Badge>
      {:else}
        <Badge>no revisions</Badge>
      {/if}
      <div class="ml-auto flex items-center gap-2">
        {#if headNumber != null}
          <Button variant="outline" onclick={() => (testOpen = true)}>Open test view</Button>
        {/if}
        {#if !showPush}
          <Button onclick={() => (showPush = true)}>Push a revision</Button>
        {/if}
      </div>
    </header>

    <PlanBanner {slug} />

    {#if headNumber != null}
      <FrontUrlDialog bind:open={testOpen} {slug} {game} number={headNumber} />
    {/if}

    {#if showPush}
      <div class="mb-8">
        <MathPushPanel
          {slug}
          {game}
          parentNumber={headNumber}
          ondone={(n) => onPushed(n)}
          oncancel={() => (showPush = false)}
        />
      </div>
    {/if}

    {#if revisions.length === 0}
      <Card class="flex flex-col items-center gap-4 border-dashed px-6 py-16 text-center">
        <span class="flex h-12 w-12 items-center justify-center rounded-xl bg-accent/10 text-accent">
          <svg
            width="22"
            height="22"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <line x1="6" y1="3" x2="6" y2="15" />
            <circle cx="18" cy="6" r="3" />
            <circle cx="6" cy="18" r="3" />
            <path d="M18 9a9 9 0 0 1-9 9" />
          </svg>
        </span>
        <div>
          <h2 class="text-lg font-semibold">No revisions yet</h2>
          <p class="mx-auto mt-1.5 max-w-md text-sm leading-relaxed text-muted">
            Revisions are immutable math snapshots. Push one straight from your browser with
            <span class="text-text">Push a revision</span> above, or run
            <span class="font-mono-tab text-text">sdt push</span> from CI.
          </p>
        </div>
        <div class="w-full max-w-xs"><CopyField value="sdt push" /></div>
      </Card>
    {:else}
      <!-- Compare picker -->
      {#if revisions.length >= 2}
        <Card class="mb-4 p-4">
          <div class="flex flex-wrap items-end gap-3">
            <span class="text-sm font-medium text-muted">Compare</span>
            <label class="flex flex-col gap-1">
              <span class="text-xs text-faint">After (newer)</span>
              <select
                bind:value={cmpAfter}
                class="h-8 rounded-md border border-border bg-surface-2 px-2 text-sm text-text outline-none transition focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
              >
                {#each revisions as r (r.number)}
                  <option value={r.number}>rev {r.number}</option>
                {/each}
              </select>
            </label>
            <span class="pb-1.5 text-faint" aria-hidden="true">↔</span>
            <label class="flex flex-col gap-1">
              <span class="text-xs text-faint">Before (older)</span>
              <select
                bind:value={cmpBefore}
                class="h-8 rounded-md border border-border bg-surface-2 px-2 text-sm text-text outline-none transition focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
              >
                {#each revisions as r (r.number)}
                  <option value={r.number}>rev {r.number}</option>
                {/each}
              </select>
            </label>
            <Button size="sm" disabled={!canCompare} onclick={compare}>Compare</Button>
          </div>
        </Card>
      {/if}

      <Card class="overflow-hidden">
        <div class="overflow-x-auto">
          <table class="w-full min-w-[46rem] text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                <th class="px-4 py-3 font-medium">Rev</th>
                <th class="px-4 py-3 font-medium">Message</th>
                <th class="px-4 py-3 font-medium">Author</th>
                <th class="px-4 py-3 font-medium">Age</th>
                <th class="px-4 py-3 font-medium text-right">Files</th>
                <th class="px-4 py-3 font-medium text-right">Size</th>
                <th class="px-4 py-3 font-medium">Stats</th>
              </tr>
            </thead>
            <tbody>
              {#each revisions as r (r.number)}
                {@const badge = statsBadge(r.stats_status)}
                <tr
                  class="cursor-pointer border-b border-border/60 transition last:border-0 hover:bg-surface-2"
                  role="link"
                  tabindex="0"
                  aria-label={`Revision ${r.number}`}
                  onclick={() => openRevision(r.number)}
                  onkeydown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      openRevision(r.number);
                    }
                  }}
                >
                  <td class="px-4 py-3 font-mono-tab font-semibold">{r.number}</td>
                  <td class="px-4 py-3">
                    <span class="line-clamp-1 max-w-[22rem] text-text">{r.message || '—'}</span>
                  </td>
                  <td class="px-4 py-3 text-muted">{r.author_display_name || '—'}</td>
                  <td class="px-4 py-3 text-muted" title={r.created_at}>{relativeAge(r.created_at)}</td>
                  <td class="px-4 py-3 text-right font-mono-tab text-muted">{r.files_count}</td>
                  <td class="px-4 py-3 text-right font-mono-tab text-muted">{humanSize(r.total_size)}</td>
                  <td class="px-4 py-3">
                    {#if badge}
                      <Badge tone={badge.tone} class={badge.pulse ? 'animate-pulse' : ''}>{badge.label}</Badge>
                    {:else}
                      <span class="text-faint">—</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </Card>
    {/if}

    <SharePanel {slug} {game} {revisions} {headNumber} />
  {/if}
</main>
