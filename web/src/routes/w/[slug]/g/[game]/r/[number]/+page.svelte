<script lang="ts">
  import { page } from '$app/state';
  import { api, ApiError, type RevisionDetail } from '$lib/api';
  import { errorText, humanSize, formatCost, formatRtp, formatMultiplier } from '$lib/format';
  import { workspaceName } from '$lib/workspaces.svelte';
  import { copyText } from '$lib/clipboard';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import FrontUrlDialog from '$lib/components/FrontUrlDialog.svelte';
  import Breadcrumbs from '$lib/components/Breadcrumbs.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';
  import Time from '$lib/components/Time.svelte';

  let slug = $derived(page.params.slug ?? '');
  let game = $derived(page.params.game ?? '');
  let numParam = $derived(page.params.number ?? '');
  let revNum = $derived(Number(numParam));

  let testOpen = $state(false);

  let detail = $state<RevisionDetail | null>(null);
  let loading = $state(true);
  let loadError = $state('');
  let notFound = $state(false);

  let statsStatus = $derived(detail?.stats?.status ?? null);
  let totalSize = $derived(detail ? detail.files.reduce((a, f) => a + f.size, 0) : 0);

  // Reload whenever the route params change (the component is reused across
  // /r/:number navigations).
  $effect(() => {
    void slug;
    void game;
    void numParam;
    void load(true);
  });

  // Poll the detail endpoint while stats are pending; stop on ok/error, on a
  // status change, and on unmount (effect teardown clears the interval).
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
      // the current view and tries again on the next tick.
      if (initial) {
        if (e instanceof ApiError && e.status === 404) notFound = true;
        else loadError = errorText(e);
      }
    } finally {
      if (initial) loading = false;
    }
  }

  // --- Short-hash copy (adapted from CopyField for inline table use) ---------
  let copiedHash = $state<string | null>(null);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  async function copyHash(hash: string) {
    const ok = await copyText(hash, 'Hash copied');
    if (ok) {
      copiedHash = hash;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copiedHash = null), 1600);
    } else {
      copiedHash = null;
    }
  }

  function shortHash(h: string): string {
    return h.length > 12 ? h.slice(0, 12) : h;
  }

  $effect(() => () => clearTimeout(copyTimer));
</script>

<svelte:head><title>rev {numParam} · {game} · Stake Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-5xl px-6 py-10">
  <Breadcrumbs
    items={[
      { label: workspaceName(slug), href: `/w/${slug}` },
      { label: game, href: `/w/${slug}/g/${game}` },
      { label: `rev ${numParam}` }
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
    <header class="mb-8">
      <div class="flex flex-wrap items-start justify-between gap-4">
        <div class="min-w-0">
          <h1 class="text-2xl font-semibold tracking-tight">
            {detail.message || `Revision ${detail.number}`}
          </h1>
          <div class="mt-2 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-sm text-muted">
            <span class="font-mono-tab text-text">rev {detail.number}</span>
            <span aria-hidden="true">·</span>
            <span>{detail.author_display_name || 'Unknown author'}</span>
            <span aria-hidden="true">·</span>
            <Time iso={detail.created_at} />
            <span aria-hidden="true">·</span>
            <span>{detail.files.length} {detail.files.length === 1 ? 'file' : 'files'}</span>
            <span aria-hidden="true">·</span>
            <span class="font-mono-tab">{humanSize(totalSize)}</span>
          </div>
        </div>
        <div class="flex flex-shrink-0 items-center gap-2">
          <Button href={`/w/${slug}/g/${game}/r/${revNum}/math`} variant="outline" size="sm">
            Math report
          </Button>
          <Button variant="outline" size="sm" onclick={() => (testOpen = true)}>
            Open test view
          </Button>
          {#if revNum > 1}
            <Button
              href={`/w/${slug}/g/${game}/diff/${revNum}/${revNum - 1}`}
              variant="outline"
              size="sm"
            >
              Compare with previous
            </Button>
          {/if}
        </div>
      </div>
    </header>

    <FrontUrlDialog bind:open={testOpen} {slug} {game} number={revNum} />

    <!-- Stats -->
    <section class="mb-10">
      <SectionHeader title="Bet stats" />
      <Card class="overflow-hidden">
        {#if statsStatus === 'pending'}
          <div class="flex items-center gap-3 px-4 py-8 text-muted">
            <span class="spinner"></span> Computing bet stats… this refreshes automatically.
          </div>
        {:else if statsStatus === 'error'}
          <div class="px-4 py-6">
            <div class="mb-2"><Badge tone="danger">stats error</Badge></div>
            <p class="text-sm text-danger">
              {detail.stats?.error || 'The server could not compute stats for this revision.'}
            </p>
          </div>
        {:else if statsStatus === 'ok' && detail.stats}
          {#if detail.stats.modes.length === 0}
            <p class="px-4 py-8 text-center text-sm text-muted">No bet modes reported.</p>
          {:else}
            <div class="overflow-x-auto">
              <table class="w-full min-w-[36rem] text-sm">
                <thead>
                  <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                    <th class="px-4 py-3 font-medium">Mode</th>
                    <th class="px-4 py-3 font-medium text-right">Cost</th>
                    <th class="px-4 py-3 font-medium text-right">RTP</th>
                    <th class="px-4 py-3 font-medium text-right">Max win</th>
                    <th class="px-4 py-3 font-medium text-right">Entries</th>
                  </tr>
                </thead>
                <tbody>
                  {#each detail.stats.modes as m (m.mode)}
                    <tr class="border-b border-border/60 last:border-0">
                      <td class="px-4 py-3 font-medium">{m.mode}</td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">{formatCost(m.cost)}</td>
                      <td class="px-4 py-3 text-right font-mono-tab">{formatRtp(m.rtp)}</td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">{formatMultiplier(m.max_win)}</td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">
                        {m.entries != null ? m.entries.toLocaleString() : '—'}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {:else}
          <p class="px-4 py-8 text-center text-sm text-muted">Stats aren't available for this revision.</p>
        {/if}
      </Card>
    </section>

    <!-- Files -->
    <section>
      <SectionHeader title={`Files · ${detail.files.length}`} />
      <Card class="overflow-hidden">
        {#if detail.files.length === 0}
          <p class="px-4 py-8 text-center text-sm text-muted">No files in this revision.</p>
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full min-w-[36rem] text-sm">
              <thead>
                <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                  <th class="px-4 py-3 font-medium">Path</th>
                  <th class="px-4 py-3 font-medium text-right">Size</th>
                  <th class="px-4 py-3 font-medium">Hash</th>
                </tr>
              </thead>
              <tbody>
                {#each detail.files as f (f.path)}
                  <tr class="border-b border-border/60 last:border-0">
                    <td class="px-4 py-3 font-mono-tab">{f.path}</td>
                    <td class="px-4 py-3 text-right font-mono-tab text-muted">{humanSize(f.size)}</td>
                    <td class="px-4 py-3">
                      <button
                        type="button"
                        onclick={() => copyHash(f.hash)}
                        title={f.hash}
                        class="inline-flex items-center gap-2 font-mono-tab text-xs text-muted transition hover:text-text"
                      >
                        <span>{shortHash(f.hash) || '—'}</span>
                        <span class="text-faint">{copiedHash === f.hash ? 'copied' : 'copy'}</span>
                      </button>
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
