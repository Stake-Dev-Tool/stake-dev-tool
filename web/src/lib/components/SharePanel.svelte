<script lang="ts">
  /**
   * SharePanel — the game page's "Share" tab (M5):
   *
   *   • Game front status — a best-effort probe of whether this game has an
   *     uploaded front bundle (the build a share serves). The bundle itself is
   *     pushed from the Revisions tab with the Push button; shares use the latest one.
   *   • Create share (owner/admin only) — pin a revision (or track latest),
   *     optional custom slug / password / expiry / session cap → POST, then
   *     prepend to the list and show the new URL prominently (stays inline).
   *   • Share links list — every ShareLink as a card: URL (CopyField, or a
   *     "no play domain" hint), rev/bundle/password/expiry/revoked badges,
   *     counters, and Revoke / Delete actions. Manual Refresh; no polling.
   *
   * Members can view the list (and copy URLs) but see no create/manage controls.
   */
  import {
    api,
    ApiError,
    isUpgradeError,
    isValidShareSlug,
    type BillingStatus,
    type CreateShareInput,
    type FrontBundleSummary,
    type Role,
    type RevisionSummary,
    type ShareLink
  } from '$lib/api';
  import { billingStatus, invalidateBillingStatus } from '$lib/billing';
  import { session } from '$lib/session.svelte';
  import { toast } from '$lib/toasts.svelte';
  import { errorText, formatExpiry, humanSize, formatRtp } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import UpgradeNotice from '$lib/components/UpgradeNotice.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import Time from '$lib/components/Time.svelte';

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

  // --- Plan limits (shared billing cache) --------------------------------------
  // Drives the expiry hint ("links last up to N days on your plan") and the
  // active-link quota line. Best-effort: a failed fetch just hides the hints.
  let billing = $state<BillingStatus | null>(null);
  $effect(() => {
    const s = slug;
    billing = null;
    if (!s) return;
    let cancelled = false;
    billingStatus(s)
      .then((r) => {
        if (!cancelled) billing = r;
      })
      .catch(() => {
        if (!cancelled) billing = null;
      });
    return () => {
      cancelled = true;
    };
  });

  /** Re-read the billing status after a mutation so the quota line stays true. */
  function refreshBilling() {
    invalidateBillingStatus(slug);
    billingStatus(slug)
      .then((r) => (billing = r))
      .catch(() => {});
  }

  /** Longest allowed link lifetime in days on this plan; null = unlimited. */
  let maxLinkDays = $derived(billing?.enabled ? billing.limits.max_share_link_days : null);
  /** Active-link cap on this plan; null = unlimited. */
  let maxLinks = $derived(billing?.enabled ? billing.limits.max_active_share_links : null);
  let usedLinks = $derived(billing?.usage.active_share_links ?? 0);
  let atLinkCap = $derived(maxLinks != null && usedLinks >= maxLinks);

  // Best-effort front-bundle status: HEAD the same-origin front route the server
  // serves an uploaded bundle at. Purely informational — a failure reads as "none".
  let frontStatus = $state<'checking' | 'present' | 'absent'>('checking');
  $effect(() => {
    const s = slug;
    const g = game;
    frontStatus = 'checking';
    let cancelled = false;
    fetch(`/api/ws/${encodeURIComponent(s)}/g/${encodeURIComponent(g)}/front/`, {
      method: 'HEAD',
      credentials: 'same-origin'
    })
      .then((r) => {
        if (!cancelled) frontStatus = r.ok ? 'present' : 'absent';
      })
      .catch(() => {
        if (!cancelled) frontStatus = 'absent';
      });
    return () => {
      cancelled = true;
    };
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

  // --- Manage bundles (owner/admin) -------------------------------------------
  // A compact, lazily-loaded expandable: the game's front bundles with a Delete
  // that frees storage. Loaded on first expand; reloads when the game changes.
  let showBundles = $state(false);
  let bundles = $state<FrontBundleSummary[]>([]);
  let loadingBundles = $state(false);
  let bundlesError = $state('');
  let bundlesLoaded = $state(false);
  let bundleBusyId = $state<string | null>(null);

  // Collapse + drop the cache when the game/workspace changes so a stale list
  // never flashes for a different game.
  $effect(() => {
    void slug;
    void game;
    showBundles = false;
    bundlesLoaded = false;
    bundles = [];
    bundlesError = '';
  });

  async function toggleBundles() {
    showBundles = !showBundles;
    if (showBundles && !bundlesLoaded) await loadBundles();
  }

  async function loadBundles() {
    loadingBundles = true;
    bundlesError = '';
    try {
      bundles = await api.games.frontBundles(slug, game);
      bundlesLoaded = true;
    } catch (e) {
      bundlesError = errorText(e);
    } finally {
      loadingBundles = false;
    }
  }

  /** Friendly copy for a bundle-delete failure (guards surface their own text). */
  function bundleErrorMessage(e: unknown): string {
    if (e instanceof ApiError) {
      switch (e.code) {
        case 'bundle_pinned':
        case 'last_bundle':
          return e.message; // lists the pinning share slugs / explains the guard
        case 'bundle_not_found':
          return 'That bundle no longer exists — refresh the list.';
        case 'network_error':
          return 'Could not reach the server. Check your connection and try again.';
      }
      return e.message || 'The bundle could not be deleted.';
    }
    return errorText(e);
  }

  async function deleteBundle(b: FrontBundleSummary) {
    if (
      !confirm(
        `Delete this front bundle? This is permanent and frees ${humanSize(b.total_size)} of storage that no share or revision still shares.`
      )
    ) {
      return;
    }
    bundlesError = '';
    bundleBusyId = b.id;
    try {
      const res = await api.games.deleteFrontBundle(slug, game, b.id);
      bundles = bundles.filter((x) => x.id !== b.id);
      // The next bundle in the (newest-first) list is now the latest one served.
      if (b.is_latest && bundles.length > 0) bundles[0] = { ...bundles[0], is_latest: true };
      toast.success(
        res.freed_blobs > 0
          ? `Bundle deleted · freed ${humanSize(res.freed_bytes)}.`
          : 'Bundle deleted · no storage freed (its files are still shared).'
      );
    } catch (e) {
      bundlesError = bundleErrorMessage(e);
    } finally {
      bundleBusyId = null;
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
  let newFeedback = $state(false);

  let creating = $state(false);
  let createError = $state('');
  let createErrorUpgrade = $state(false);
  /** The just-created link, shown in a prominent copy callout above the list. */
  let createdShare = $state<ShareLink | null>(null);

  let slugInvalid = $derived(newSlug.trim().length > 0 && !isValidShareSlug(newSlug.trim()));
  /** Caught client-side so the plan cap reads as guidance, not a server error. */
  let expiryTooLong = $derived.by(() => {
    if (maxLinkDays == null) return false;
    const days = Number.parseInt(newExpiryDays.trim(), 10);
    return newExpiryDays.trim() !== '' && Number.isFinite(days) && days > maxLinkDays;
  });
  let canCreate = $derived(!creating && !slugInvalid && !expiryTooLong);

  function resetCreateForm() {
    newSlug = '';
    newRev = 'latest';
    newPassword = '';
    newExpiryDays = '';
    newMaxSessions = '25';
    newFeedback = false;
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
      if (newFeedback) input.feedback_enabled = true;

      const created = await api.shares.create(slug, game, input);
      shares = [created, ...shares];
      createdShare = created;
      resetCreateForm();
      showCreate = false;
      refreshBilling();
    } catch (e) {
      createError = shareErrorMessage(e);
      createErrorUpgrade = isUpgradeError(e);
    } finally {
      creating = false;
    }
  }

  async function toggleFeedback(s: ShareLink) {
    actionError = '';
    busyId = s.id;
    try {
      const updated = await api.shares.update(slug, game, s.id, {
        feedback_enabled: !s.feedback_enabled
      });
      shares = shares.map((x) => (x.id === s.id ? updated : x));
      toast.success(
        updated.feedback_enabled
          ? `Feedback enabled on ${s.slug} — visitors now get the feedback overlay.`
          : `Feedback disabled on ${s.slug}.`
      );
    } catch (e) {
      actionError = shareErrorMessage(e);
    } finally {
      busyId = null;
    }
  }

  async function revokeShare(s: ShareLink) {
    if (!confirm(`Revoke ${s.slug}? Visitors lose access to this link immediately.`)) return;
    actionError = '';
    busyId = s.id;
    try {
      const updated = await api.shares.revoke(slug, game, s.id);
      shares = shares.map((x) => (x.id === s.id ? updated : x));
      toast.success(`Share link ${s.slug} revoked.`);
      refreshBilling();
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
      toast.success(`Share link ${s.slug} deleted.`);
      refreshBilling();
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

<section>
  <SectionHeader title="Share" class="mb-4">
    Host a playable instance of this game and share the link. Analytics land back here.
    {#snippet action()}
      <Button variant="outline" size="sm" onclick={refresh} loading={loadingShares}>Refresh</Button>
    {/snippet}
  </SectionHeader>

  <!-- Game front status -->
  <Card class="mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 p-4">
    <div class="flex items-center gap-2.5 text-sm">
      <span
        class="h-2 w-2 rounded-full {frontStatus === 'present'
          ? 'bg-accent'
          : frontStatus === 'checking'
            ? 'bg-warn'
            : 'bg-faint'}"
      ></span>
      <span class="font-medium text-text">Game front</span>
      {#if frontStatus === 'checking'}
        <span class="text-muted">checking…</span>
      {:else if frontStatus === 'present'}
        <Badge tone="accent">bundle uploaded</Badge>
      {:else}
        <span class="text-muted">no bundle uploaded yet</span>
      {/if}
    </div>
    <span class="text-xs text-faint">
      Push or update it from the Revisions tab with the Push button. Shares serve the latest bundle.
    </span>
  </Card>

  <!-- Manage bundles (owner/admin): compact, lazily-loaded expandable -------->
  {#if canManage}
    <div class="mb-4">
      <button
        type="button"
        class="flex w-full items-center gap-2 text-left text-sm text-muted transition hover:text-text"
        aria-expanded={showBundles}
        onclick={toggleBundles}
      >
        <span class="text-xs text-faint">{showBundles ? '▾' : '▸'}</span>
        Manage bundles
        <span class="text-xs text-faint">— delete old builds to free storage</span>
      </button>

      {#if showBundles}
        <div class="mt-2">
          {#if loadingBundles && bundles.length === 0}
            <Card class="p-4"><Skeleton /></Card>
          {:else if bundlesError}
            <Card class="p-4">
              <p class="text-sm text-danger">{bundlesError}</p>
              <Button variant="outline" size="sm" class="mt-3" onclick={loadBundles}>Retry</Button>
            </Card>
          {:else if bundles.length === 0}
            <Card class="p-4">
              <p class="text-sm text-muted">No front bundles uploaded for this game yet.</p>
            </Card>
          {:else}
            <Card class="divide-y divide-border/60 p-0">
              {#each bundles as b (b.id)}
                <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-4 py-3">
                  <span class="font-mono-tab text-xs text-muted">{shortId(b.id)}</span>
                  {#if b.is_latest}
                    <Badge tone="accent">latest</Badge>
                  {/if}
                  <span class="text-xs text-faint">created <Time iso={b.created_at} /></span>
                  <span class="text-xs text-muted">{b.files_count} files · {humanSize(b.total_size)}</span>
                  <div class="ml-auto">
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={bundleBusyId === b.id}
                      onclick={() => deleteBundle(b)}
                    >
                      Delete
                    </Button>
                  </div>
                </div>
              {/each}
            </Card>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  <!-- 2) Create share (owner/admin) ----------------------------------------->
  {#if canManage}
    {#if !showCreate}
      <div class="mb-4 flex flex-wrap items-center gap-3">
        <Button variant="secondary" onclick={() => (showCreate = true)}>New share link</Button>
        {#if maxLinks != null}
          <span class="text-xs text-muted">
            {usedLinks} of {maxLinks} active share {maxLinks === 1 ? 'link' : 'links'} used on
            your plan
          </span>
        {/if}
      </div>
      {#if atLinkCap}
        <div class="mb-4">
          <UpgradeNotice
            {slug}
            message={`Your plan allows ${maxLinks} active share ${maxLinks === 1 ? 'link' : 'links'} — revoke or delete one to free the slot, or upgrade for more.`}
          />
        </div>
      {/if}
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
                label={maxLinkDays != null ? `Expires (days, max ${maxLinkDays})` : 'Expires (days)'}
                bind:value={newExpiryDays}
                inputmode="numeric"
                placeholder={maxLinkDays != null ? String(maxLinkDays) : 'Never'}
                disabled={creating}
                error={expiryTooLong
                  ? `Links last up to ${maxLinkDays} days on your plan — upgrade for longer.`
                  : undefined}
                hint={maxLinkDays != null
                  ? `Free links last up to ${maxLinkDays} days (the default if left blank).`
                  : undefined}
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

            <label class="flex items-start gap-2.5 sm:col-span-2">
              <input
                type="checkbox"
                bind:checked={newFeedback}
                disabled={creating}
                class="mt-0.5 h-4 w-4 accent-accent"
              />
              <span class="flex flex-col gap-0.5">
                <span class="text-sm font-medium text-muted">Enable visitor feedback</span>
                <span class="text-xs text-faint">
                  Injects a feedback overlay into the game: testers can write notes and draw on
                  the screen, each entry tagged with the exact round they just played.
                </span>
              </span>
            </label>
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
    <Card class="p-6"><Skeleton /></Card>
  {:else if sharesError}
    <Card class="p-6">
      <p class="text-sm text-danger">{sharesError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={refresh}>Retry</Button>
    </Card>
  {:else if shares.length === 0}
    <EmptyState title="No share links yet">
      {#if canManage}
        Create a share link above to hand this game to a tester — no install, just a URL.
      {:else}
        No one has created a share link for this game yet.
      {/if}
    </EmptyState>
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
              {#if s.feedback_enabled}
                <Badge tone="accent">💬 feedback</Badge>
              {/if}
              <span class="ml-auto text-xs text-faint">created <Time iso={s.created_at} /></span>
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
                <span class="text-danger">Revoked <Time iso={s.revoked_at} /></span>
              {/if}
              {#if canManage}
                <div class="ml-auto flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busyId === s.id}
                    onclick={() => toggleFeedback(s)}
                  >
                    {s.feedback_enabled ? 'Disable feedback' : 'Enable feedback'}
                  </Button>
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
