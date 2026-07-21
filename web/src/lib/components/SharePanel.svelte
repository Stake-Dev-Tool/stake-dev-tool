<script lang="ts">
  /**
   * SharePanel — the game page's "Share" section (M5). Three stacked pieces:
   *
   *   1. Game front card — upload the front build a share serves. Reuses
   *      MathFolderPicker (root `index.html`, ≤ 2000 files) + `runFrontPush`
   *      (the same hash → check → upload → commit pipeline as math), with a
   *      compact inline progress recap.
   *   2. Create share (owner/admin only) — pin a revision (or track latest),
   *      optional custom slug / password / expiry / session cap → POST, then
   *      prepend to the list and show the new URL prominently.
   *   3. Share links list — every ShareLinkView as a card: URL (CopyField, or a
   *      "no play domain" hint), rev/bundle/password/expiry/revoked badges,
   *      counters, and Revoke / Delete actions. Manual Refresh; no polling.
   *
   * Members can view the list (and copy URLs) but see no create/manage controls.
   */
  import {
    api,
    ApiError,
    isUpgradeError,
    isValidShareSlug,
    type CreateShareInput,
    type Role,
    type RevisionSummary,
    type ShareLink
  } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { errorText, formatExpiry, humanSize, relativeAge, formatRtp } from '$lib/format';
  import {
    runFrontPush,
    pushErrorMessage,
    type FileProgress,
    type IntakeFile,
    type PushPhase
  } from '$lib/push';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import MathFolderPicker from '$lib/components/MathFolderPicker.svelte';
  import UpgradeNotice from '$lib/components/UpgradeNotice.svelte';

  type Props = {
    slug: string;
    game: string;
    /** The game's revisions (already loaded on the page) — drives the pin select. */
    revisions: RevisionSummary[];
    /** The game's head revision number, or null when it has none yet. */
    headNumber: number | null;
  };

  let { slug, game, revisions, headNumber }: Props = $props();

  // --- Load: shares list + the caller's role (to gate manage controls) --------
  let role = $state<Role | null>(null);
  let canManage = $derived(role === 'owner' || role === 'admin');

  let shares = $state<ShareLink[]>([]);
  let loadingShares = $state(true);
  let sharesError = $state('');
  let actionError = $state('');
  let busyId = $state<string | null>(null);

  // Reload when the game (or workspace) changes — the page reuses this component.
  $effect(() => {
    void slug;
    void game;
    load();
  });

  async function load() {
    loadingShares = true;
    sharesError = '';
    try {
      const [detail, list] = await Promise.all([
        api.workspaces.get(slug),
        api.shares.list(slug, game)
      ]);
      role =
        detail.role ??
        detail.members.find((m) => m.user_id === (session.user?.id ?? ''))?.role ??
        null;
      shares = list;
    } catch (e) {
      sharesError = errorText(e);
    } finally {
      loadingShares = false;
    }
  }

  async function refresh() {
    loadingShares = true;
    sharesError = '';
    try {
      shares = await api.shares.list(slug, game);
    } catch (e) {
      sharesError = errorText(e);
    } finally {
      loadingShares = false;
    }
  }

  /** Friendly copy for a share create/manage failure. */
  function shareErrorMessage(e: unknown): string {
    if (e instanceof ApiError) {
      switch (e.code) {
        case 'slug_taken':
          return 'That slug is already taken — choose another.';
        case 'invalid_slug':
          return 'That slug isn’t valid. Use 2–40 chars: a–z, 0–9, hyphens (not at the ends).';
        case 'slug_generation_failed':
          return 'Could not allocate a unique slug — please try again.';
        case 'revision_not_found':
          return 'That revision no longer exists — reload and retry.';
        case 'bundle_not_found':
          return 'That front bundle no longer exists — reload and retry.';
        case 'upgrade_required':
          return e.message || 'This plan’s active share-link limit is reached — upgrade for more.';
        case 'network_error':
          return 'Could not reach the server. Check your connection and try again.';
      }
      return e.message || 'The share link operation failed.';
    }
    return errorText(e);
  }

  // --- Create share -----------------------------------------------------------
  let showCreate = $state(false);
  let newSlug = $state('');
  // Revision pin: 'latest' tracks head; a number pins that revision.
  let newRev = $state<number | 'latest'>('latest');
  let newPassword = $state('');
  let newExpiryDays = $state('');
  let newMaxSessions = $state('25');

  let creating = $state(false);
  let createError = $state('');
  let createErrorUpgrade = $state(false);
  /** The just-created link, shown in a prominent copy callout above the list. */
  let createdShare = $state<ShareLink | null>(null);

  let slugInvalid = $derived(newSlug.trim().length > 0 && !isValidShareSlug(newSlug.trim()));
  let canCreate = $derived(!creating && !slugInvalid);

  function resetCreateForm() {
    newSlug = '';
    newRev = 'latest';
    newPassword = '';
    newExpiryDays = '';
    newMaxSessions = '25';
  }

  async function createShare() {
    if (!canCreate) return;
    creating = true;
    createError = '';
    createErrorUpgrade = false;
    try {
      const input: CreateShareInput = {};
      const s = newSlug.trim();
      if (s) input.slug = s;
      if (newRev !== 'latest') input.revision_number = newRev;
      if (newPassword.length > 0) input.password = newPassword;
      const days = Number.parseInt(newExpiryDays.trim(), 10);
      if (newExpiryDays.trim() !== '' && Number.isFinite(days) && days > 0) {
        input.expires_in_days = days;
      }
      const sessions = Number.parseInt(newMaxSessions.trim(), 10);
      if (Number.isFinite(sessions) && sessions > 0) input.max_concurrent_sessions = sessions;

      const created = await api.shares.create(slug, game, input);
      shares = [created, ...shares];
      createdShare = created;
      resetCreateForm();
      showCreate = false;
    } catch (e) {
      createError = shareErrorMessage(e);
      createErrorUpgrade = isUpgradeError(e);
    } finally {
      creating = false;
    }
  }

  async function revokeShare(s: ShareLink) {
    if (!confirm(`Revoke ${s.slug}? Visitors lose access to this link immediately.`)) return;
    actionError = '';
    busyId = s.id;
    try {
      const updated = await api.shares.revoke(slug, game, s.id);
      shares = shares.map((x) => (x.id === s.id ? updated : x));
    } catch (e) {
      actionError = shareErrorMessage(e);
    } finally {
      busyId = null;
    }
  }

  async function removeShare(s: ShareLink) {
    if (!confirm(`Delete ${s.slug}? This permanently removes the link and its analytics.`)) return;
    actionError = '';
    busyId = s.id;
    try {
      await api.shares.remove(slug, game, s.id);
      shares = shares.filter((x) => x.id !== s.id);
      if (createdShare?.id === s.id) createdShare = null;
    } catch (e) {
      actionError = shareErrorMessage(e);
    } finally {
      busyId = null;
    }
  }

  // --- Per-share view helpers -------------------------------------------------
  function shortId(id: string): string {
    return id.length > 8 ? id.slice(0, 8) : id;
  }
  function isExpired(s: ShareLink): boolean {
    return s.expires_at != null && new Date(s.expires_at).getTime() < Date.now();
  }
  type StatusInfo = { label: string; tone: 'neutral' | 'accent' | 'warn' | 'danger' };
  function statusOf(s: ShareLink): StatusInfo {
    if (s.revoked_at) return { label: 'Revoked', tone: 'danger' };
    if (isExpired(s)) return { label: 'Expired', tone: 'warn' };
    return { label: 'Active', tone: 'accent' };
  }
</script>

<section class="mt-10">
  <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
    <div>
      <h2 class="text-lg font-semibold tracking-tight">Share</h2>
      <p class="mt-0.5 text-sm text-muted">
        Host a playable instance of this game and share the link. Analytics land back here. Upload the game front with the Push front button above; shares use the latest bundle.
      </p>
    </div>
    <Button variant="outline" size="sm" onclick={refresh} loading={loadingShares}>Refresh</Button>
  </div>

  <!-- 2) Create share (owner/admin) ----------------------------------------->
  {#if canManage}
    {#if !showCreate}
      <div class="mb-4">
        <Button variant="secondary" onclick={() => (showCreate = true)}>New share link</Button>
      </div>
    {:else}
      <Card class="mb-4 p-6">
        <form
          class="flex flex-col gap-5"
          onsubmit={(e) => {
            e.preventDefault();
            void createShare();
          }}
        >
          <div class="flex items-center justify-between gap-3">
            <h3 class="text-base font-semibold">New share link</h3>
            <button
              type="button"
              class="text-sm text-muted transition hover:text-text disabled:opacity-50"
              disabled={creating}
              onclick={() => (showCreate = false)}
            >
              Cancel
            </button>
          </div>

          <div class="grid gap-4 sm:grid-cols-2">
            <label class="flex flex-col gap-1.5">
              <span class="text-sm font-medium text-muted">Revision</span>
              <select
                bind:value={newRev}
                disabled={creating}
                class="h-9 rounded-md border border-border bg-surface-2 px-3 text-sm text-text outline-none transition focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
              >
                <option value="latest">Latest (tracks head{headNumber != null ? ` · rev ${headNumber}` : ''})</option>
                {#each revisions as r (r.number)}
                  <option value={r.number}>rev {r.number}</option>
                {/each}
              </select>
            </label>

            <Input
              id="share-slug"
              label="Custom slug (optional)"
              bind:value={newSlug}
              mono
              placeholder="auto-generated"
              disabled={creating}
              error={slugInvalid ? 'Use 2–40 chars: a–z, 0–9, hyphens (not at the ends).' : undefined}
              hint="The subdomain label — leave blank for a generated one."
            />

            <Input
              id="share-password"
              label="Password (optional)"
              bind:value={newPassword}
              placeholder="No password"
              disabled={creating}
              hint="Visitors must enter it before playing."
            />

            <div class="grid grid-cols-2 gap-4">
              <Input
                id="share-expiry"
                label="Expires (days)"
                bind:value={newExpiryDays}
                inputmode="numeric"
                placeholder="Never"
                disabled={creating}
              />
              <Input
                id="share-sessions"
                label="Max sessions"
                bind:value={newMaxSessions}
                inputmode="numeric"
                placeholder="25"
                disabled={creating}
              />
            </div>
          </div>

          {#if revisions.length === 0}
            <p class="rounded-md border border-warn/30 bg-warn/10 px-3 py-2 text-xs text-warn">
              This game has no revisions yet — a share needs one to serve. Push a revision first.
            </p>
          {/if}

          {#if createError}
            {#if createErrorUpgrade}
              <UpgradeNotice {slug} message={createError} />
            {:else}
              <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
                {createError}
              </p>
            {/if}
          {/if}

          <div>
            <Button type="submit" loading={creating} disabled={!canCreate}>Create share link</Button>
          </div>
        </form>
      </Card>
    {/if}

    {#if createdShare}
      <Card class="mb-4 border-accent/40 p-5">
        <div class="mb-2 flex items-center gap-2 text-sm font-medium text-accent">
          <span aria-hidden="true">✓</span> Share link created
        </div>
        {#if createdShare.url}
          <CopyField value={createdShare.url} />
        {:else}
          <p class="text-sm text-muted">
            <span class="font-mono-tab text-text">{createdShare.slug}</span> — no play domain is
            configured on this instance, so it has no public URL yet.
          </p>
        {/if}
      </Card>
    {/if}
  {/if}

  {#if actionError}
    <p class="mb-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
      {actionError}
    </p>
  {/if}

  <!-- 3) Share links list --------------------------------------------------->
  {#if loadingShares && shares.length === 0}
    <div class="flex items-center gap-3 py-8 text-muted"><span class="spinner"></span> Loading share links…</div>
  {:else if sharesError}
    <Card class="p-6">
      <p class="text-sm text-danger">{sharesError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={refresh}>Retry</Button>
    </Card>
  {:else if shares.length === 0}
    <Card class="flex flex-col items-center gap-2 border-dashed px-6 py-12 text-center">
      <h3 class="text-base font-semibold">No share links yet</h3>
      <p class="max-w-sm text-sm text-muted">
        {canManage
          ? 'Create a share link above to hand this game to a tester — no install, just a URL.'
          : 'No one has created a share link for this game yet.'}
      </p>
    </Card>
  {:else}
    <div class="flex flex-col gap-3">
      {#each shares as s (s.id)}
        {@const st = statusOf(s)}
        {@const revoked = s.revoked_at != null}
        <Card class="p-5 {revoked ? 'opacity-70' : ''}">
          <div class="flex flex-col gap-4">
            <!-- Header: slug + status/config badges -->
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-mono-tab text-sm font-semibold text-text">{s.slug}</span>
              <Badge tone={st.tone}>{st.label}</Badge>
              {#if s.revision_number != null}
                <Badge tone="accent">rev {s.revision_number}</Badge>
              {:else}
                <Badge>latest rev</Badge>
              {/if}
              {#if s.front_bundle_id != null}
                <Badge>bundle {shortId(s.front_bundle_id)}</Badge>
              {:else}
                <Badge>latest bundle</Badge>
              {/if}
              {#if s.password_protected}
                <Badge tone="warn">🔒 password</Badge>
              {/if}
              <span class="ml-auto text-xs text-faint" title={s.created_at}>
                created {relativeAge(s.created_at)}
              </span>
            </div>

            <!-- URL -->
            {#if s.url}
              <CopyField value={s.url} />
            {:else}
              <p class="rounded-md border border-border bg-surface-2/50 px-3 py-2 text-xs text-muted">
                No play domain configured on this instance — this link has no public URL yet.
              </p>
            {/if}

            <!-- Counters -->
            <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <div>
                <div class="font-mono-tab text-lg text-text">{s.sessions_count.toLocaleString()}</div>
                <div class="text-xs text-faint">sessions</div>
              </div>
              <div>
                <div class="font-mono-tab text-lg text-text">{s.spins_count.toLocaleString()}</div>
                <div class="text-xs text-faint">spins</div>
              </div>
              <div>
                <div class="font-mono-tab text-lg text-text">{formatRtp(s.observed_rtp)}</div>
                <div class="text-xs text-faint">observed RTP</div>
              </div>
              <div>
                <div class="font-mono-tab text-lg text-text">{s.active_sessions.toLocaleString()}</div>
                <div class="text-xs text-faint">active now</div>
              </div>
            </div>

            <!-- Meta + actions -->
            <div class="flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-border/60 pt-3 text-xs text-muted">
              <span>Expires: <span class="text-text">{formatExpiry(s.expires_at)}</span></span>
              <span>Session cap: <span class="text-text">{s.max_concurrent_sessions}</span></span>
              {#if revoked}
                <span class="text-danger">Revoked {relativeAge(s.revoked_at)}</span>
              {/if}
              {#if canManage}
                <div class="ml-auto flex items-center gap-2">
                  {#if !revoked}
                    <Button
                      variant="danger"
                      size="sm"
                      disabled={busyId === s.id}
                      onclick={() => revokeShare(s)}
                    >
                      Revoke
                    </Button>
                  {/if}
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={busyId === s.id}
                    onclick={() => removeShare(s)}
                  >
                    Delete
                  </Button>
                </div>
              {/if}
            </div>
          </div>
        </Card>
      {/each}
    </div>
  {/if}
</section>
