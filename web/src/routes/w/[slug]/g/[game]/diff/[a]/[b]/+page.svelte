<script lang="ts">
  import { page } from '$app/state';
  import { api, ApiError, type RevisionDiff, type DiffMode } from '$lib/api';
  import { errorText, humanSize, formatCost, formatRtp, formatMultiplier, formatRtpDelta } from '$lib/format';
  import { workspaceName } from '$lib/workspaces.svelte';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import Breadcrumbs from '$lib/components/Breadcrumbs.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';

  let slug = $derived(page.params.slug ?? '');
  let game = $derived(page.params.game ?? '');
  // a = after (:number), b = before (:other).
  let a = $derived(page.params.a ?? '');
  let b = $derived(page.params.b ?? '');

  let diff = $state<RevisionDiff | null>(null);
  let loading = $state(true);
  let loadError = $state('');
  let notFound = $state(false);

  let hasStats = $derived((diff?.stats.modes.length ?? 0) > 0);

  $effect(() => {
    void slug;
    void game;
    void a;
    void b;
    load();
  });

  async function load() {
    loading = true;
    loadError = '';
    notFound = false;
    try {
      diff = await api.games.diff(slug, game, a, b);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) notFound = true;
      else loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  function rtpDelta(m: DiffMode): { text: string; cls: string } {
    if (m.before && m.after) {
      const d = m.after.rtp - m.before.rtp;
      const cls = d > 0 ? 'text-accent' : d < 0 ? 'text-danger' : 'text-faint';
      return { text: `${formatRtpDelta(m.before.rtp, m.after.rtp)} pp`, cls };
    }
    if (m.after) return { text: 'new', cls: 'text-accent' };
    if (m.before) return { text: 'removed', cls: 'text-danger' };
    return { text: '—', cls: 'text-faint' };
  }
</script>

<svelte:head><title>Diff rev {b} → rev {a} · {game} · Stake Dev Tool Cloud</title></svelte:head>

{#snippet beforeAfter(
  before: number | null | undefined,
  after: number | null | undefined,
  fmt: (n: number) => string
)}
  {#if before != null && after != null}
    <span class="text-muted">{fmt(before)}</span>
    <span class="px-1 text-faint" aria-hidden="true">→</span>
    <span class="text-text">{fmt(after)}</span>
  {:else if after != null}
    <span class="text-text">{fmt(after)}</span>
  {:else if before != null}
    <span class="text-muted line-through">{fmt(before)}</span>
  {:else}
    <span class="text-faint">—</span>
  {/if}
{/snippet}

<main class="mx-auto w-full max-w-5xl px-6 py-10">
  <Breadcrumbs
    items={[
      { label: workspaceName(slug), href: `/w/${slug}` },
      { label: game, href: `/w/${slug}/g/${game}` },
      { label: `rev ${b} → rev ${a}` }
    ]}
  />

  {#if loading}
    <Card class="p-6"><Skeleton /></Card>
  {:else if notFound}
    <EmptyState title="Diff not available">
      One of these revisions doesn't exist, or you don't have access to this game.
      {#snippet cta()}
        <Button href={`/w/${slug}/g/${game}`} variant="outline">Back to revisions</Button>
      {/snippet}
    </EmptyState>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={load}>Retry</Button>
    </Card>
  {:else if diff}
    <header class="mb-8 flex flex-wrap items-center gap-3">
      <h1 class="text-2xl font-semibold tracking-tight">Compare</h1>
      <div class="flex items-center gap-2 text-sm">
        <Badge>rev {b}</Badge>
        <span class="text-faint" aria-hidden="true">→</span>
        <Badge tone="accent">rev {a}</Badge>
      </div>
    </header>

    <!-- File summary chips -->
    <section class="mb-8">
      <SectionHeader title="Files" />
      <div class="mb-4 flex flex-wrap gap-2">
        <Badge tone="accent">+{diff.files.added.length} added</Badge>
        <Badge tone="danger">−{diff.files.removed.length} removed</Badge>
        <Badge tone="warn">~{diff.files.changed.length} changed</Badge>
        <Badge tone="neutral">{diff.files.unchanged} unchanged</Badge>
      </div>

      {#if diff.files.added.length + diff.files.removed.length + diff.files.changed.length === 0}
        <Card class="p-6"><p class="text-sm text-muted">No file changes between these revisions.</p></Card>
      {:else}
        <div class="flex flex-col gap-4">
          {#if diff.files.changed.length > 0}
            <Card class="overflow-hidden">
              <div class="border-b border-border px-4 py-2.5 text-xs font-semibold uppercase tracking-wide text-faint">
                Changed
              </div>
              <div class="overflow-x-auto">
                <table class="w-full min-w-[34rem] text-sm">
                  <thead>
                    <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                      <th class="px-4 py-3 font-medium">Path</th>
                      <th class="px-4 py-3 font-medium text-right">Size</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each diff.files.changed as file (file.path)}
                      <tr class="border-b border-border/60 last:border-0">
                        <td class="px-4 py-3 font-mono-tab">{file.path}</td>
                        <td class="px-4 py-3 text-right font-mono-tab">
                          {@render beforeAfter(file.before_size, file.after_size, humanSize)}
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </Card>
          {/if}

          {#if diff.files.added.length > 0}
            <Card class="overflow-hidden">
              <div class="border-b border-border px-4 py-2.5 text-xs font-semibold uppercase tracking-wide text-accent">
                Added
              </div>
              <div class="overflow-x-auto">
                <table class="w-full min-w-[34rem] text-sm">
                  <thead>
                    <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                      <th class="px-4 py-3 font-medium">Path</th>
                      <th class="px-4 py-3 font-medium text-right">Size</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each diff.files.added as file (file.path)}
                      <tr class="border-b border-border/60 last:border-0">
                        <td class="px-4 py-3 font-mono-tab">{file.path}</td>
                        <td class="px-4 py-3 text-right font-mono-tab text-muted">{humanSize(file.size)}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </Card>
          {/if}

          {#if diff.files.removed.length > 0}
            <Card class="overflow-hidden">
              <div class="border-b border-border px-4 py-2.5 text-xs font-semibold uppercase tracking-wide text-danger">
                Removed
              </div>
              <div class="overflow-x-auto">
                <table class="w-full min-w-[34rem] text-sm">
                  <thead>
                    <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                      <th class="px-4 py-3 font-medium">Path</th>
                      <th class="px-4 py-3 font-medium text-right">Size</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each diff.files.removed as file (file.path)}
                      <tr class="border-b border-border/60 last:border-0">
                        <td class="px-4 py-3 font-mono-tab text-muted line-through">{file.path}</td>
                        <td class="px-4 py-3 text-right font-mono-tab text-muted">{humanSize(file.size)}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </Card>
          {/if}
        </div>
      {/if}
    </section>

    <!-- Stats diff -->
    <section>
      <SectionHeader title="Bet stats" />
      <Card class="overflow-hidden">
        {#if !hasStats}
          <p class="px-4 py-8 text-center text-sm text-muted">
            Stats comparison isn't available — neither revision has computed bet stats.
          </p>
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full min-w-[42rem] text-sm">
              <thead>
                <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                  <th class="px-4 py-3 font-medium">Mode</th>
                  <th class="px-4 py-3 font-medium">Cost</th>
                  <th class="px-4 py-3 font-medium">RTP</th>
                  <th class="px-4 py-3 font-medium text-right">Δ RTP</th>
                  <th class="px-4 py-3 font-medium">Max win</th>
                </tr>
              </thead>
              <tbody>
                {#each diff.stats.modes as m (m.mode)}
                  {@const d = rtpDelta(m)}
                  <tr class="border-b border-border/60 last:border-0">
                    <td class="px-4 py-3 font-medium">
                      {m.mode}
                      {#if m.after && !m.before}<Badge tone="accent" class="ml-1">new</Badge>{/if}
                      {#if m.before && !m.after}<Badge tone="danger" class="ml-1">removed</Badge>{/if}
                    </td>
                    <td class="px-4 py-3 font-mono-tab">
                      {@render beforeAfter(m.before?.cost, m.after?.cost, formatCost)}
                    </td>
                    <td class="px-4 py-3 font-mono-tab">
                      {@render beforeAfter(m.before?.rtp, m.after?.rtp, formatRtp)}
                    </td>
                    <td class="px-4 py-3 text-right font-mono-tab font-medium {d.cls}">{d.text}</td>
                    <td class="px-4 py-3 font-mono-tab">
                      {@render beforeAfter(m.before?.max_win, m.after?.max_win, formatMultiplier)}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </Card>
    </section>
  {/if}
</main>
