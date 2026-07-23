<script lang="ts">
  /**
   * FeedbackPanel — the game page's "Feedback" tab: every visitor feedback
   * entry across the game's share links, newest first.
   *
   *   • Each card: who/when/which link, the written note, the annotation
   *     rendered over the captured screenshot (FeedbackSketch), and the round
   *     it references — the book line `(revision, mode, eventId)` the visitor
   *     had just played, shown as badges.
   *   • Filter by share link; manual Refresh; Delete for owner/admin.
   *
   * Submission happens on the share host (the overlay the share link injects
   * when its feedback toggle is on — see SharePanel).
   */
  import { api, type Role, type ShareFeedback } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { toast } from '$lib/toasts.svelte';
  import { errorText } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import FeedbackSketch from '$lib/components/FeedbackSketch.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import Time from '$lib/components/Time.svelte';

  type Props = {
    slug: string;
    game: string;
  };

  let { slug, game }: Props = $props();

  let role = $state<Role | null>(null);
  let canManage = $derived(role === 'owner' || role === 'admin');

  let entries = $state<ShareFeedback[]>([]);
  let loading = $state(true);
  let loadError = $state('');
  let actionError = $state('');
  let busyId = $state<string | null>(null);

  /** Share-link filter: 'all' or a share slug present in the loaded entries. */
  let filter = $state<string>('all');
  let shareSlugs = $derived([...new Set(entries.map((e) => e.share_slug))].sort());
  let visible = $derived(
    filter === 'all' ? entries : entries.filter((e) => e.share_slug === filter)
  );

  // Reload when the game (or workspace) changes — the page reuses this component.
  $effect(() => {
    void slug;
    void game;
    filter = 'all';
    load();
  });

  async function load() {
    loading = true;
    loadError = '';
    try {
      const [detail, list] = await Promise.all([
        api.workspaces.get(slug),
        api.feedback.list(slug, game)
      ]);
      role =
        detail.role ??
        detail.members.find((m) => m.user_id === (session.user?.id ?? ''))?.role ??
        null;
      entries = list;
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  async function refresh() {
    loading = true;
    loadError = '';
    try {
      entries = await api.feedback.list(slug, game);
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  async function remove(entry: ShareFeedback) {
    if (!confirm('Delete this feedback entry? This is permanent.')) return;
    actionError = '';
    busyId = entry.id;
    try {
      await api.feedback.remove(slug, game, entry.id);
      entries = entries.filter((e) => e.id !== entry.id);
      toast.success('Feedback entry deleted.');
    } catch (e) {
      actionError = errorText(e);
    } finally {
      busyId = null;
    }
  }
</script>

<section>
  <SectionHeader title="Feedback" class="mb-4">
    What share-link visitors reported — notes and on-screen annotations, each tied to the exact
    round they had just played. Enable feedback per link from the Share tab.
    {#snippet action()}
      <Button variant="outline" size="sm" onclick={refresh} loading={loading}>Refresh</Button>
    {/snippet}
  </SectionHeader>

  {#if actionError}
    <p class="mb-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
      {actionError}
    </p>
  {/if}

  {#if loading && entries.length === 0}
    <Card class="p-6"><Skeleton /></Card>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={load}>Retry</Button>
    </Card>
  {:else if entries.length === 0}
    <EmptyState title="No feedback yet">
      Nothing has been submitted on this game's share links. Turn on
      <span class="text-text">Enable visitor feedback</span> for a link in the Share tab — visitors
      then get a feedback button and a drawing overlay right in the game.
    </EmptyState>
  {:else}
    {#if shareSlugs.length > 1}
      <div class="mb-4 flex items-center gap-2">
        <label for="feedback-filter" class="text-sm text-muted">Share link</label>
        <select
          id="feedback-filter"
          bind:value={filter}
          class="h-8 rounded-md border border-border bg-surface-2 px-2 text-sm text-text outline-none transition focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
        >
          <option value="all">All ({entries.length})</option>
          {#each shareSlugs as s (s)}
            <option value={s}>{s}</option>
          {/each}
        </select>
      </div>
    {/if}

    <div class="flex flex-col gap-3">
      {#each visible as entry (entry.id)}
        <Card class="p-5">
          <div class="flex flex-col gap-3">
            <!-- Header: author + link + round reference -->
            <div class="flex flex-wrap items-center gap-2">
              <span class="text-sm font-semibold text-text">
                {entry.author_name || 'Anonymous'}
              </span>
              <Badge>{entry.share_slug}</Badge>
              {#if entry.mode != null && entry.event_id != null}
                <span title="The last round played when this was sent — mode · book event id">
                  <Badge tone="accent">{entry.mode} · #{entry.event_id}</Badge>
                </span>
                {#if entry.revision_number != null}
                  <span title="Revision the link was serving">
                    <Badge>rev {entry.revision_number}</Badge>
                  </span>
                {/if}
              {:else}
                <span title="Sent before the first spin of the session">
                  <Badge tone="warn">no round</Badge>
                </span>
              {/if}
              <span class="ml-auto text-xs text-faint"><Time iso={entry.created_at} /></span>
            </div>

            <!-- Message -->
            {#if entry.message}
              <p class="whitespace-pre-wrap text-sm text-text">{entry.message}</p>
            {/if}

            <!-- Annotation -->
            {#if entry.drawing && entry.drawing.shapes.length > 0}
              <div class="max-w-xl">
                <FeedbackSketch
                  shapes={entry.drawing.shapes}
                  width={entry.viewport_w ?? 1280}
                  height={entry.viewport_h ?? 720}
                  screenshotUrl={entry.has_screenshot
                    ? api.feedback.screenshotUrl(slug, game, entry.id)
                    : null}
                />
              </div>
            {/if}

            <!-- Actions -->
            {#if canManage}
              <div class="flex items-center border-t border-border/60 pt-3">
                <div class="ml-auto">
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={busyId === entry.id}
                    onclick={() => remove(entry)}
                  >
                    Delete
                  </Button>
                </div>
              </div>
            {/if}
          </div>
        </Card>
      {/each}
    </div>
  {/if}
</section>
