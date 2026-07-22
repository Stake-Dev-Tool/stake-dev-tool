<script lang="ts">
  import { page } from '$app/state';
  import {
    api,
    ApiError,
    type RevisionDetail,
    type RevisionAnalysis,
    type ConstraintRow,
    type ModeAnalysis
  } from '$lib/api';
  import { errorText, formatMetric } from '$lib/format';
  import { workspaceName } from '$lib/workspaces.svelte';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import Breadcrumbs from '$lib/components/Breadcrumbs.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';
  import Time from '$lib/components/Time.svelte';
  import MathVerdict from '$lib/components/MathVerdict.svelte';
  import MathConstraints from '$lib/components/MathConstraints.svelte';
  import MathModePanel from '$lib/components/MathModePanel.svelte';

  let slug = $derived(page.params.slug ?? '');
  let game = $derived(page.params.game ?? '');
  let numParam = $derived(page.params.number ?? '');
  let revNum = $derived(Number(numParam));

  let detail = $state<RevisionDetail | null>(null);
  let loading = $state(true);
  let loadError = $state('');
  let notFound = $state(false);

  let stats = $derived(detail?.stats ?? null);
  let statsStatus = $derived(stats?.status ?? null);
  let analysis = $derived<RevisionAnalysis | null>(stats?.analysis ?? null);
  let constraints = $derived<ConstraintRow[]>(analysis?.constraints ?? []);
  let modes = $derived<ModeAnalysis[]>(analysis?.modes ?? []);

  // Selected mode for the mode workspace. Defaults to the cost-1 base mode (falls
  // back to the first) once the analysis arrives, and resets if the user
  // navigates to a revision whose modes don't include it.
  let selectedModeName = $state<string | null>(null);
  $effect(() => {
    const ms = modes;
    if (ms.length === 0) {
      if (selectedModeName !== null) selectedModeName = null;
      return;
    }
    if (!selectedModeName || !ms.some((m) => m.mode === selectedModeName)) {
      selectedModeName = (ms.find((m) => m.cost === 1) ?? ms[0]).mode;
    }
  });

  // Reload whenever the route params change (the component is reused across
  // /r/:number navigations).
  $effect(() => {
    void slug;
    void game;
    void numParam;
    void load(true);
  });

  // Poll while stats are pending so the report appears the moment the analyzer
  // finishes; the effect teardown stops the poll on ok/error and on unmount.
  $effect(() => {
    if (statsStatus !== 'pending') return;
    const id = setInterval(() => void load(false), 3000);
    return () => clearInterval(id);
  });

  async function load(initial: boolean) {
    if (initial) {
      loading = true;
      loadError = '';
      notFound = false;
    }
    try {
      detail = await api.games.revision(slug, game, numParam);
    } catch (e) {
      // Only surface errors on the initial load — a failed background poll keeps
      // the current view and retries on the next tick.
      if (initial) {
        if (e instanceof ApiError && e.status === 404) notFound = true;
        else loadError = errorText(e);
      }
    } finally {
      if (initial) loading = false;
    }
  }
</script>

<svelte:head><title>Math report · rev {numParam} · {game} · Stake Dev Tool Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-5xl px-6 py-10">
  <Breadcrumbs
    items={[
      { label: workspaceName(slug), href: `/w/${slug}` },
      { label: game, href: `/w/${slug}/g/${game}` },
      { label: `rev ${numParam}`, href: `/w/${slug}/g/${game}/r/${revNum}` },
      { label: 'Math' }
    ]}
  />

  {#if loading}
    <Card class="p-6"><Skeleton /></Card>
  {:else if notFound}
    <EmptyState title="Revision not found">
      This revision doesn't exist, or you don't have access to it.
      {#snippet cta()}
        <Button href={`/w/${slug}/g/${game}`} variant="outline">Back to revisions</Button>
      {/snippet}
    </EmptyState>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={() => load(true)}>Retry</Button>
    </Card>
  {:else if detail}
    <!-- Page header -->
    <header class="mb-6">
      <div class="flex flex-wrap items-baseline gap-x-2.5 gap-y-1">
        <h1 class="text-2xl font-semibold tracking-tight">Math report</h1>
        <span class="text-sm text-muted">{game}</span>
        <span aria-hidden="true" class="text-faint">·</span>
        <span class="font-mono-tab text-sm text-text">rev {detail.number}</span>
        <span aria-hidden="true" class="text-faint">·</span>
        <Time iso={detail.created_at} class="text-sm text-muted" />
      </div>
    </header>

    {#if analysis}
      <!-- 1 · Verdict first, problems first -->
      <MathVerdict {analysis} {constraints} />

      <!-- Sticky mini-nav: sits under the app header, repeats the verdict small so
           it never scrolls away. z below the app nav (z-10). -->
      <nav
        class="sticky top-14 z-[9] -mx-6 mb-8 flex items-center gap-4 border-b border-border bg-bg/85 px-6 py-2 text-sm backdrop-blur"
      >
        <a href="#constraints" class="text-muted transition hover:text-text">Constraints</a>
        {#if modes.length > 0}
          <a href="#modes" class="text-muted transition hover:text-text">Modes</a>
        {/if}
        <div class="ml-auto flex items-center gap-1.5">
          <span
            class="rounded-full border px-1.5 py-0.5 text-xs font-medium {analysis.two_star_compliant
              ? 'border-accent/30 bg-accent/10 text-accent'
              : 'border-danger/30 bg-danger/10 text-danger'}"
          >
            2★
          </span>
          <span
            class="rounded-full border px-1.5 py-0.5 text-xs font-medium {analysis.three_star_compliant
              ? 'border-accent/30 bg-accent/10 text-accent'
              : 'border-danger/30 bg-danger/10 text-danger'}"
          >
            3★
          </span>
        </div>
      </nav>

      <!-- 2 · Bet-level constraints -->
      <section id="constraints" class="mb-10 scroll-mt-28">
        <SectionHeader title="Bet-level compliance">
          {#snippet children()}
            Evaluated at the reference max bets — 2★
            <span class="font-mono-tab text-text">{formatMetric(analysis.reference_max_bet_2)}</span>
            · 3★
            <span class="font-mono-tab text-text">{formatMetric(analysis.reference_max_bet_3)}</span>.
            Failing rows sort first.
          {/snippet}
        </SectionHeader>
        <MathConstraints
          {constraints}
          referenceMaxBet2={analysis.reference_max_bet_2}
          referenceMaxBet3={analysis.reference_max_bet_3}
        />
      </section>

      <!-- 3 · Mode workspace -->
      {#if modes.length > 0}
        <MathModePanel {modes} {selectedModeName} onselect={(m) => (selectedModeName = m)} />
      {/if}
    {:else if statsStatus === 'pending'}
      <Card class="flex items-center gap-3 px-4 py-10 text-muted">
        <span class="spinner"></span> Computing the compliance report… this refreshes automatically.
      </Card>
    {:else if statsStatus === 'error'}
      <Card class="px-4 py-6">
        <div class="mb-2"><Badge tone="danger">stats error</Badge></div>
        <p class="text-sm text-danger">
          {stats?.error || 'The server could not compute stats for this revision.'}
        </p>
      </Card>
    {:else}
      <!-- analysis absent: older revision predating the analyzer -->
      <EmptyState title="No compliance analysis yet">
        This revision predates the compliance analyzer, so it has no Math report. Push a new revision
        to recompute it — the report appears here automatically once the analysis finishes.
        {#snippet cta()}
          <Button href={`/w/${slug}/g/${game}`} variant="outline">Back to game</Button>
        {/snippet}
      </EmptyState>
    {/if}
  {/if}
</main>
