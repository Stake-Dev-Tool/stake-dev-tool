<script lang="ts">
  import { api, type RevisionSummary, type FrontBundleSummary } from '$lib/api';
  import { errorText, humanSize } from '$lib/format';
  import Badge from './Badge.svelte';
  import Button from './Button.svelte';
  import Card from './Card.svelte';
  import Time from './Time.svelte';

  let { slug, game, revisions = [], refresh = 0, preview = false }: {
    slug: string;
    game: string;
    revisions?: RevisionSummary[];
    refresh?: number;
    preview?: boolean;
  } = $props();
  let bundles = $state<FrontBundleSummary[]>([]);
  let loading = $state(true);
  let error = $state('');
  let retry = $state(0);
  $effect(() => {
    const s = slug, g = game;
    void refresh;
    void retry;
    let cancelled = false;
    bundles = [];
    error = '';
    loading = true;
    api.games.frontBundles(s, g)
      .then((result) => { if (!cancelled) bundles = result; })
      .catch((e) => { if (!cancelled) error = errorText(e); })
      .finally(() => { if (!cancelled) loading = false; });
    return () => { cancelled = true; };
  });
  let rows = $derived([
    ...revisions.map((revision) => ({ type: 'Math' as const, id: String(revision.number), created_at: revision.created_at, revision })),
    ...bundles.map((bundle) => ({ type: 'Front' as const, id: bundle.id, created_at: bundle.created_at, bundle }))
  ].sort((a, b) => (Date.parse(b.created_at) || 0) - (Date.parse(a.created_at) || 0) || `${a.type}:${a.id}`.localeCompare(`${b.type}:${b.id}`)));
  let visibleRows = $derived(preview ? rows.slice(0, 1) : rows);
  let apiBase = $derived(`/api/workspaces/${encodeURIComponent(slug)}/games/${encodeURIComponent(game)}`);
</script>

<p class="mb-4 text-sm text-muted">
  Math revisions and frontend builds are independent uploads. Front builds use bundle IDs, not revision numbers.
</p>
{#if loading}
  <p class="mb-4 text-sm text-muted" role="status">Loading front builds…</p>
{:else if error}
  <div class="mb-4 rounded-md border border-danger/30 p-3" role="alert">
    <p class="text-sm text-danger">Could not load front builds: {error}</p>
    <Button size="sm" variant="outline" class="mt-2" onclick={() => retry++}>Retry front builds</Button>
  </div>
{:else if bundles.length === 0}
  <p class="mb-4 text-sm text-muted">No front builds yet. Upload a frontend build with an index.html entry point.</p>
{/if}
{#if preview && bundles.length > 1}
  <p class="mb-4 text-xs text-muted">Showing the latest front build. Open All game builds for more.</p>
{:else if bundles.length >= 50}
  <p class="mb-4 text-xs text-muted">Showing the 50 most recent front builds returned by the server.</p>
{/if}
{#if rows.length > 0}
  <Card class="overflow-hidden">
    <div class="overflow-x-auto">
      <table aria-label="Build history" class="w-full min-w-[52rem] text-sm">
        <thead>
          <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
            {#each ['Type', 'Build', 'Message / author', 'Age', 'Files', 'Size', 'Stats', 'Actions'] as label}
              <th class="px-4 py-3 font-medium">{label}</th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each visibleRows as row (`${row.type}:${row.id}`)}
            <tr class="border-b border-border/60 last:border-0 hover:bg-surface-2">
              <td class="px-4 py-3"><Badge tone={row.type === 'Math' ? 'accent' : 'neutral'}>{row.type}</Badge></td>
              {#if row.type === 'Math'}
                {@const r = row.revision}
                <td class="px-4 py-3 font-mono-tab"><a class="text-accent hover:underline" href={`/w/${slug}/g/${game}/r/${r.number}`}>rev {r.number}</a></td>
                <td class="px-4 py-3"><span class="line-clamp-2 max-w-56">{r.message || '—'}</span><span class="text-xs text-muted">{r.author_display_name || '—'}</span></td>
                <td class="px-4 py-3 text-muted"><Time iso={r.created_at} /></td>
                <td class="px-4 py-3 font-mono-tab text-muted">{r.files_count}</td>
                <td class="px-4 py-3 font-mono-tab text-muted">{humanSize(r.total_size)}</td>
                <td class="px-4 py-3 text-muted">{r.stats_status === 'pending' ? 'computing' : r.stats_status === 'ok' ? 'stats ok' : r.stats_status === 'error' ? 'stats error' : '—'}</td>
                <td class="px-4 py-3"><Button href={`${apiBase}/revisions/${r.number}/download`} download data-sveltekit-reload variant="outline" size="sm">Download math build</Button></td>
              {:else}
                {@const b = row.bundle}
                <td class="px-4 py-3"><code class="block max-w-44 break-all text-xs">{b.id}</code>{#if b.is_latest}<Badge class="mt-1">latest front</Badge>{/if}</td>
                <td class="px-4 py-3 text-muted">Frontend bundle</td>
                <td class="px-4 py-3 text-muted"><Time iso={b.created_at} /></td>
                <td class="px-4 py-3 font-mono-tab text-muted">{b.files_count}</td>
                <td class="px-4 py-3 font-mono-tab text-muted">{humanSize(b.total_size)}</td>
                <td class="px-4 py-3 text-muted">Not applicable</td>
                <td class="px-4 py-3"><Button href={`${apiBase}/front-bundles/${encodeURIComponent(b.id)}/download`} download data-sveltekit-reload variant="outline" size="sm">Download front build</Button></td>
              {/if}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </Card>
{/if}
