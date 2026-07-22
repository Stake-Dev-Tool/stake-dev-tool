<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    ApiError,
    type AdminOverview,
    type AdminWorkspace,
    type AdminUser,
    type AdminShare,
    type AdminOverrideInput,
    type AdminOverridePlan
  } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { isAdmin } from '$lib/admin';
  import { toast } from '$lib/toasts.svelte';
  import { humanSize, formatDate, errorText } from '$lib/format';
  import { planLabel, statusLabel, daysUntil } from '$lib/billing';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import Breadcrumbs from '$lib/components/Breadcrumbs.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';
  import Tabs from '$lib/components/Tabs.svelte';
  import Time from '$lib/components/Time.svelte';
  import Sparkline from '$lib/components/Sparkline.svelte';

  // ---- Admin gate --------------------------------------------------------
  // Non-admins 404 on EVERY /admin endpoint; the probe resolves that to `false`.
  // A false probe (or any 404 that slips through later) renders the standard
  // not-found EmptyState — never an error banner.
  let checking = $state(true);
  let notFound = $state(false);

  /** Any 404 anywhere in the admin surface means "not an admin" → not-found. */
  function is404(e: unknown): boolean {
    return e instanceof ApiError && e.status === 404;
  }

  // ---- Overview ----------------------------------------------------------
  let overview = $state<AdminOverview | null>(null);
  let overviewLoading = $state(true);
  let overviewError = $state('');

  let tiles = $derived(
    overview
      ? [
          { label: 'Users', value: overview.users.toLocaleString() },
          { label: 'Workspaces', value: overview.workspaces.toLocaleString() },
          { label: 'Games', value: overview.games.toLocaleString() },
          { label: 'Revisions', value: overview.revisions.toLocaleString() },
          { label: 'Share links', value: overview.share_links.toLocaleString() },
          { label: 'Storage', value: humanSize(overview.storage_bytes) },
          { label: 'Sessions', value: overview.sessions_total.toLocaleString() },
          { label: 'Spins', value: overview.spins_total.toLocaleString() }
        ]
      : []
  );
  let signupsTotal = $derived(
    overview ? overview.signups_30d.reduce((s, d) => s + d.count, 0) : 0
  );
  let pushesTotal = $derived(overview ? overview.pushes_30d.reduce((s, d) => s + d.count, 0) : 0);

  // ---- Tabs (deep-linkable via #workspaces / #users / #shares) -----------
  type AdminTab = 'workspaces' | 'users' | 'shares';
  let activeTab = $state<AdminTab>('workspaces');
  function selectTab(id: string) {
    activeTab = id as AdminTab;
    if (typeof history !== 'undefined') history.replaceState(history.state, '', `#${id}`);
    ensureTab(activeTab);
  }
  function ensureTab(t: AdminTab) {
    if (notFound) return;
    if (t === 'workspaces' && !wsLoaded && !wsLoading) void loadWs();
    if (t === 'users' && !usersLoaded && !usersLoading) void loadUsers();
    if (t === 'shares' && !sharesLoaded && !sharesLoading) void loadShares();
  }

  /** 300ms trailing debounce — one instance per search box. */
  function debounce(fn: () => void, ms = 300): () => void {
    let t: ReturnType<typeof setTimeout>;
    return () => {
      clearTimeout(t);
      t = setTimeout(fn, ms);
    };
  }

  // ---- Workspaces tab ----------------------------------------------------
  let wsQuery = $state('');
  let wsRows = $state<AdminWorkspace[]>([]);
  let wsLoading = $state(false);
  let wsLoaded = $state(false);
  let wsError = $state('');
  const wsSearch = debounce(() => void loadWs());

  async function loadWs() {
    wsLoading = true;
    wsError = '';
    try {
      wsRows = await api.admin.workspaces(wsQuery);
      wsLoaded = true;
    } catch (e) {
      if (is404(e)) {
        notFound = true;
        return;
      }
      wsError = errorText(e);
    } finally {
      wsLoading = false;
    }
  }

  function planTone(plan: string): 'neutral' | 'accent' | 'info' | 'danger' {
    if (plan === 'unlimited') return 'info';
    if (plan === 'solo' || plan === 'team') return 'accent';
    if (plan === 'free') return 'danger';
    return 'neutral'; // anything unexpected
  }

  // Plan-override (comp) editor — an inline panel expanded under a row.
  type MgPlan = 'none' | 'solo' | 'team' | 'unlimited';
  let managingId = $state<string | null>(null);
  let mgPlan = $state<MgPlan>('none');
  let mgExpiry = $state('');
  let mgNote = $state('');
  let mgBusy = $state(false);
  let mgError = $state('');

  function openManage(w: AdminWorkspace) {
    if (managingId === w.id) {
      managingId = null;
      return;
    }
    managingId = w.id;
    mgError = '';
    mgBusy = false;
    const op = w.override;
    mgPlan =
      op && (op.plan === 'solo' || op.plan === 'team' || op.plan === 'unlimited')
        ? op.plan
        : 'none';
    // Seed the expiry box from the override's remaining days (blank = no expiry).
    mgExpiry = op?.expires_at ? String(daysUntil(op.expires_at)) : '';
    mgNote = op?.note ?? '';
  }

  async function saveOverride(w: AdminWorkspace) {
    if (mgBusy) return;
    mgBusy = true;
    mgError = '';
    const plan: AdminOverridePlan = mgPlan === 'none' ? null : mgPlan;
    const input: AdminOverrideInput = { plan };
    if (plan) {
      const days = mgExpiry.trim();
      if (days !== '') {
        const n = Number(days);
        if (Number.isFinite(n) && n > 0) input.expires_in_days = n;
      }
      const note = mgNote.trim();
      if (note !== '') input.note = note;
    }
    try {
      const updated = await api.admin.setOverride(w.id, input);
      wsRows = wsRows.map((r) => (r.id === w.id ? updated : r));
      managingId = null;
      toast.success(plan ? `Comp set: ${planLabel(plan)}` : 'Comp cleared');
    } catch (e) {
      if (is404(e)) {
        notFound = true;
        return;
      }
      mgError = errorText(e);
    } finally {
      mgBusy = false;
    }
  }

  // ---- Users tab ---------------------------------------------------------
  let usersQuery = $state('');
  let usersRows = $state<AdminUser[]>([]);
  let usersLoading = $state(false);
  let usersLoaded = $state(false);
  let usersError = $state('');
  let usersBusy = $state<string | null>(null);
  const usersSearch = debounce(() => void loadUsers());

  async function loadUsers() {
    usersLoading = true;
    usersError = '';
    try {
      usersRows = await api.admin.users(usersQuery);
      usersLoaded = true;
    } catch (e) {
      if (is404(e)) {
        notFound = true;
        return;
      }
      usersError = errorText(e);
    } finally {
      usersLoading = false;
    }
  }

  async function toggleAdmin(u: AdminUser) {
    const makeAdmin = !u.is_admin;
    if (!makeAdmin) {
      const who = u.display_name || u.email;
      if (!confirm(`Remove admin from ${who}? They'll lose access to the admin console.`)) return;
    }
    usersBusy = u.id;
    try {
      const result = await api.admin.setAdmin(u.id, makeAdmin);
      usersRows = usersRows.map((x) => (x.id === u.id ? { ...x, is_admin: result } : x));
      toast.success(makeAdmin ? 'Admin granted' : 'Admin removed');
    } catch (e) {
      if (e instanceof ApiError && e.code === 'last_admin') {
        toast.error("Can't remove the last admin — grant another admin first.");
      } else if (is404(e)) {
        notFound = true;
      } else {
        toast.error(errorText(e));
      }
    } finally {
      usersBusy = null;
    }
  }

  // ---- Shares tab --------------------------------------------------------
  let sharesQuery = $state('');
  let sharesRows = $state<AdminShare[]>([]);
  let sharesLoading = $state(false);
  let sharesLoaded = $state(false);
  let sharesError = $state('');
  let sharesBusy = $state<string | null>(null);
  const sharesSearch = debounce(() => void loadShares());

  async function loadShares() {
    sharesLoading = true;
    sharesError = '';
    try {
      sharesRows = await api.admin.shares(sharesQuery);
      sharesLoaded = true;
    } catch (e) {
      if (is404(e)) {
        notFound = true;
        return;
      }
      sharesError = errorText(e);
    } finally {
      sharesLoading = false;
    }
  }

  async function revokeShare(s: AdminShare) {
    if (s.revoked_at) return;
    if (!confirm(`Revoke share "${s.slug}"? Its link stops working immediately.`)) return;
    sharesBusy = s.id;
    try {
      await api.admin.revokeShare(s.id);
      sharesRows = sharesRows.map((x) =>
        x.id === s.id ? { ...x, revoked_at: new Date().toISOString() } : x
      );
      toast.success('Share revoked');
    } catch (e) {
      if (is404(e)) {
        notFound = true;
      } else {
        toast.error(errorText(e));
      }
    } finally {
      sharesBusy = null;
    }
  }

  // ---- Boot --------------------------------------------------------------
  onMount(async () => {
    const h = location.hash.replace('#', '');
    if (h === 'workspaces' || h === 'users' || h === 'shares') activeTab = h;

    let ok = false;
    try {
      ok = await isAdmin();
    } catch {
      ok = false;
    }
    checking = false;
    if (!ok) {
      notFound = true;
      overviewLoading = false;
      return;
    }
    await loadOverview();
    ensureTab(activeTab);
  });

  async function loadOverview() {
    overviewLoading = true;
    overviewError = '';
    try {
      overview = await api.admin.overview();
    } catch (e) {
      if (is404(e)) {
        notFound = true;
        return;
      }
      overviewError = errorText(e);
    } finally {
      overviewLoading = false;
    }
  }
</script>

<svelte:head><title>Admin · Stake Dev Tool Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-6xl px-6 py-10">
  <Breadcrumbs items={[{ label: 'Admin' }]} />

  {#if checking}
    <Card class="p-6"><Skeleton /></Card>
  {:else if notFound}
    <EmptyState title="Page not found">
      This page doesn't exist, or you don't have access to it.
      {#snippet cta()}
        <Button href="/" variant="outline">Back to workspaces</Button>
      {/snippet}
    </EmptyState>
  {:else}
    <header class="mb-6">
      <h1 class="text-2xl font-semibold tracking-tight">Admin</h1>
      <p class="mt-1 text-sm text-muted">
        Instance-wide stats, plan comps, users and share moderation.
      </p>
    </header>

    <!-- Overview -->
    <section class="mb-8">
      {#if overviewLoading}
        <Card class="p-6"><Skeleton /></Card>
      {:else if overviewError}
        <Card class="p-6">
          <p class="text-sm text-danger">{overviewError}</p>
          <Button variant="outline" size="sm" class="mt-4" onclick={loadOverview}>Retry</Button>
        </Card>
      {:else if overview}
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          {#each tiles as t (t.label)}
            <Card class="p-4">
              <div class="text-xs font-medium uppercase tracking-wide text-faint">{t.label}</div>
              <div class="mt-1 font-mono-tab text-2xl font-semibold tracking-tight">{t.value}</div>
            </Card>
          {/each}
        </div>

        <div class="mt-4 grid gap-3 sm:grid-cols-2">
          <Card class="p-4">
            <div class="mb-2 flex items-baseline justify-between gap-2">
              <span class="text-xs font-medium uppercase tracking-wide text-faint">
                New signups
              </span>
              <span class="text-xs text-muted">
                <span class="font-mono-tab text-text">{signupsTotal.toLocaleString()}</span> · 30 days
              </span>
            </div>
            <Sparkline data={overview.signups_30d} label="signups" />
          </Card>
          <Card class="p-4">
            <div class="mb-2 flex items-baseline justify-between gap-2">
              <span class="text-xs font-medium uppercase tracking-wide text-faint">
                Math pushes
              </span>
              <span class="text-xs text-muted">
                <span class="font-mono-tab text-text">{pushesTotal.toLocaleString()}</span> · 30 days
              </span>
            </div>
            <Sparkline data={overview.pushes_30d} label="pushes" />
          </Card>
        </div>

        {#if overview.host}
          {@const h = overview.host}
          {@const diskUsed = h.disk_total_bytes - h.disk_free_bytes}
          {@const diskPct = h.disk_total_bytes > 0 ? diskUsed / h.disk_total_bytes : 0}
          {@const memPct = h.mem_total_bytes > 0 ? h.mem_used_bytes / h.mem_total_bytes : 0}
          <Card class="mt-4 p-4">
            <div class="mb-3 flex items-baseline justify-between gap-2">
              <span class="text-xs font-medium uppercase tracking-wide text-faint">
                Machine
              </span>
              <span class="text-xs text-muted">scale signal — disk backing the blob store</span>
            </div>
            <div class="grid gap-4 sm:grid-cols-2">
              {#each [{ label: 'Disk', used: diskUsed, total: h.disk_total_bytes, pct: diskPct, hint: `${humanSize(h.disk_free_bytes)} free` }, { label: 'Memory', used: h.mem_used_bytes, total: h.mem_total_bytes, pct: memPct, hint: `${humanSize(h.mem_total_bytes - h.mem_used_bytes)} free` }] as g (g.label)}
                <div>
                  <div class="mb-1 flex items-baseline justify-between text-sm">
                    <span class="text-muted">{g.label}</span>
                    <span class="font-mono-tab text-text">
                      {humanSize(g.used)} / {humanSize(g.total)}
                      <span class="text-faint">· {Math.round(g.pct * 100)}%</span>
                    </span>
                  </div>
                  <div class="h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
                    <div
                      class="h-full rounded-full transition-all {g.pct >= 0.9
                        ? 'bg-danger'
                        : g.pct >= 0.75
                          ? 'bg-warn'
                          : 'bg-accent'}"
                      style="width: {Math.min(100, Math.round(g.pct * 100))}%"
                    ></div>
                  </div>
                  <p class="mt-1 text-xs text-faint">{g.hint}</p>
                </div>
              {/each}
            </div>
          </Card>
        {/if}
      {/if}
    </section>

    <!-- Management tabs -->
    <Tabs
      class="mb-6"
      tabs={[
        { id: 'workspaces', label: 'Workspaces', badge: wsLoaded ? wsRows.length : undefined },
        { id: 'users', label: 'Users', badge: usersLoaded ? usersRows.length : undefined },
        { id: 'shares', label: 'Shares', badge: sharesLoaded ? sharesRows.length : undefined }
      ]}
      active={activeTab}
      onselect={selectTab}
    />

    {#if activeTab === 'workspaces'}
      <section>
        <SectionHeader title="Workspaces">
          Manage plan overrides — grant a <span class="text-text">comp</span> subscription, or clear
          a comp back to the normal billing state.
        </SectionHeader>

        <div class="mb-4 max-w-sm">
          <Input
            bind:value={wsQuery}
            oninput={wsSearch}
            placeholder="Search slug or name…"
            aria-label="Search workspaces"
            spellcheck={false}
            autocomplete="off"
          />
        </div>

        {#if wsError}
          <Card class="p-6">
            <p class="text-sm text-danger">{wsError}</p>
            <Button variant="outline" size="sm" class="mt-3" onclick={loadWs}>Retry</Button>
          </Card>
        {:else if wsLoading && !wsLoaded}
          <Card class="p-6"><Skeleton /></Card>
        {:else if wsRows.length === 0}
          <EmptyState title={wsQuery.trim() ? 'No matching workspaces' : 'No workspaces yet'}>
            {wsQuery.trim()
              ? 'No workspace matches that search. Try a different slug or name.'
              : 'When teams create workspaces they show up here for plan management.'}
          </EmptyState>
        {:else}
          <Card class="overflow-hidden">
            <div class="overflow-x-auto">
              <table class="w-full min-w-[60rem] text-sm">
                <thead>
                  <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                    <th class="px-4 py-3 font-medium">Workspace</th>
                    <th class="px-4 py-3 font-medium">Created</th>
                    <th class="px-4 py-3 font-medium text-right">Members</th>
                    <th class="px-4 py-3 font-medium text-right">Games</th>
                    <th class="px-4 py-3 font-medium text-right">Storage</th>
                    <th class="px-4 py-3 font-medium">Plan</th>
                    <th class="px-4 py-3 font-medium text-right">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {#each wsRows as w (w.id)}
                    <tr class="border-b border-border/60 last:border-0">
                      <td class="px-4 py-3">
                        <div class="font-medium">{w.name}</div>
                        <div class="mt-0.5 font-mono-tab text-xs text-muted">{w.slug}</div>
                      </td>
                      <td class="px-4 py-3 text-muted"><Time iso={w.created_at} /></td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">{w.members}</td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">{w.games}</td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">
                        {humanSize(w.storage_bytes)}
                      </td>
                      <td class="px-4 py-3">
                        <div class="flex flex-col items-start gap-1">
                          <Badge tone={planTone(w.plan)}>{planLabel(w.plan)}</Badge>
                          {#if w.override}
                            <span class="text-xs text-warn">
                              comped: {w.override.plan}
                              {#if w.override.expires_at}
                                → {formatDate(w.override.expires_at)}
                              {:else}
                                · no expiry
                              {/if}
                            </span>
                          {/if}
                          {#if w.subscription_status}
                            <span class="text-xs text-faint">{statusLabel(w.subscription_status)}</span>
                          {/if}
                        </div>
                      </td>
                      <td class="px-4 py-3 text-right">
                        <Button
                          variant={managingId === w.id ? 'secondary' : 'outline'}
                          size="sm"
                          onclick={() => openManage(w)}
                        >
                          {managingId === w.id ? 'Close' : 'Manage plan'}
                        </Button>
                      </td>
                    </tr>
                    {#if managingId === w.id}
                      <tr class="border-b border-border/60 bg-surface-2/40 last:border-0">
                        <td colspan="7" class="px-4 py-4">
                          <div class="fade-in">
                            <div class="mb-3 text-sm font-medium text-text">
                              Plan override (comp) · <span class="font-mono-tab text-muted">{w.slug}</span>
                            </div>
                            <div class="flex flex-wrap items-end gap-3">
                              <label class="flex flex-col gap-1.5">
                                <span class="text-sm font-medium text-muted">Plan</span>
                                <select
                                  bind:value={mgPlan}
                                  class="h-9 rounded-md border border-border bg-surface-2 px-3 text-sm text-text outline-none transition focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
                                >
                                  <option value="none">None (clear comp)</option>
                                  <option value="solo">Solo</option>
                                  <option value="team">Team</option>
                                  <option value="unlimited">Unlimited</option>
                                </select>
                              </label>
                              <Input
                                label="Expires in (days)"
                                type="number"
                                min="1"
                                bind:value={mgExpiry}
                                placeholder="no expiry"
                                hint="Blank = no expiry"
                                disabled={mgPlan === 'none'}
                                class="w-40"
                              />
                              <Input
                                label="Note"
                                bind:value={mgNote}
                                placeholder="e.g. launch partner"
                                disabled={mgPlan === 'none'}
                                class="min-w-[14rem] flex-1"
                              />
                              <Button loading={mgBusy} onclick={() => saveOverride(w)}>
                                {mgPlan === 'none' ? 'Clear comp' : 'Apply comp'}
                              </Button>
                            </div>
                            {#if mgError}
                              <p class="mt-3 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
                                {mgError}
                              </p>
                            {/if}
                            <p class="mt-3 text-xs text-faint">
                              A comp overrides Polar billing: the workspace runs on the chosen plan
                              (or Unlimited) until it expires or you clear it. "None" removes the comp
                              and returns the workspace to its normal billing state.
                            </p>
                          </div>
                        </td>
                      </tr>
                    {/if}
                  {/each}
                </tbody>
              </table>
            </div>
          </Card>
        {/if}
      </section>
    {:else if activeTab === 'users'}
      <section>
        <SectionHeader title="Users">
          Grant or remove instance-admin access. The last remaining admin can't be removed.
        </SectionHeader>

        <div class="mb-4 max-w-sm">
          <Input
            bind:value={usersQuery}
            oninput={usersSearch}
            placeholder="Search email or name…"
            aria-label="Search users"
            spellcheck={false}
            autocomplete="off"
          />
        </div>

        {#if usersError}
          <Card class="p-6">
            <p class="text-sm text-danger">{usersError}</p>
            <Button variant="outline" size="sm" class="mt-3" onclick={loadUsers}>Retry</Button>
          </Card>
        {:else if usersLoading && !usersLoaded}
          <Card class="p-6"><Skeleton /></Card>
        {:else if usersRows.length === 0}
          <EmptyState title={usersQuery.trim() ? 'No matching users' : 'No users yet'}>
            {usersQuery.trim()
              ? 'No user matches that search. Try a different email or name.'
              : 'Registered users appear here.'}
          </EmptyState>
        {:else}
          <Card class="overflow-hidden">
            <div class="overflow-x-auto">
              <table class="w-full min-w-[48rem] text-sm">
                <thead>
                  <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                    <th class="px-4 py-3 font-medium">User</th>
                    <th class="px-4 py-3 font-medium">Created</th>
                    <th class="px-4 py-3 font-medium text-right">Workspaces</th>
                    <th class="px-4 py-3 font-medium">Role</th>
                    <th class="px-4 py-3 font-medium text-right">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {#each usersRows as u (u.id)}
                    {@const self = u.id === session.user?.id}
                    <tr class="border-b border-border/60 last:border-0">
                      <td class="px-4 py-3">
                        <div class="flex items-center gap-2">
                          <span class="font-medium">{u.display_name || '—'}</span>
                          {#if self}<Badge>you</Badge>{/if}
                        </div>
                        <div class="mt-0.5 text-xs text-faint">{u.email}</div>
                      </td>
                      <td class="px-4 py-3 text-muted"><Time iso={u.created_at} /></td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">{u.workspaces}</td>
                      <td class="px-4 py-3">
                        {#if u.is_admin}
                          <Badge tone="info">admin</Badge>
                        {:else}
                          <span class="text-xs text-faint">—</span>
                        {/if}
                      </td>
                      <td class="px-4 py-3 text-right">
                        <Button
                          variant={u.is_admin ? 'danger' : 'outline'}
                          size="sm"
                          disabled={usersBusy === u.id}
                          onclick={() => toggleAdmin(u)}
                        >
                          {u.is_admin ? 'Remove admin' : 'Make admin'}
                        </Button>
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          </Card>
        {/if}
      </section>
    {:else}
      <section>
        <SectionHeader title="Share links">
          Every hosted share link across all workspaces. Revoke to kill a link immediately.
        </SectionHeader>

        <div class="mb-4 max-w-sm">
          <Input
            bind:value={sharesQuery}
            oninput={sharesSearch}
            placeholder="Search slug, workspace or game…"
            aria-label="Search shares"
            spellcheck={false}
            autocomplete="off"
          />
        </div>

        {#if sharesError}
          <Card class="p-6">
            <p class="text-sm text-danger">{sharesError}</p>
            <Button variant="outline" size="sm" class="mt-3" onclick={loadShares}>Retry</Button>
          </Card>
        {:else if sharesLoading && !sharesLoaded}
          <Card class="p-6"><Skeleton /></Card>
        {:else if sharesRows.length === 0}
          <EmptyState title={sharesQuery.trim() ? 'No matching shares' : 'No share links yet'}>
            {sharesQuery.trim()
              ? 'No share link matches that search.'
              : 'Hosted share links from every workspace appear here.'}
          </EmptyState>
        {:else}
          <Card class="overflow-hidden">
            <div class="overflow-x-auto">
              <table class="w-full min-w-[60rem] text-sm">
                <thead>
                  <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                    <th class="px-4 py-3 font-medium">Share</th>
                    <th class="px-4 py-3 font-medium">Workspace</th>
                    <th class="px-4 py-3 font-medium">Game</th>
                    <th class="px-4 py-3 font-medium text-right">Sessions</th>
                    <th class="px-4 py-3 font-medium text-right">Spins</th>
                    <th class="px-4 py-3 font-medium">Created</th>
                    <th class="px-4 py-3 font-medium">Status</th>
                    <th class="px-4 py-3 font-medium text-right">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {#each sharesRows as s (s.id)}
                    <tr class="border-b border-border/60 last:border-0">
                      <td class="px-4 py-3">
                        <div class="font-mono-tab font-medium">{s.slug}</div>
                        {#if s.url}
                          <div class="mt-1.5 max-w-[24rem]"><CopyField value={s.url} /></div>
                        {:else}
                          <div class="mt-0.5 text-xs text-faint">no play domain configured</div>
                        {/if}
                      </td>
                      <td class="px-4 py-3 font-mono-tab text-muted">{s.workspace_slug}</td>
                      <td class="px-4 py-3 font-mono-tab text-muted">{s.game}</td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">
                        {s.sessions_count.toLocaleString()}
                      </td>
                      <td class="px-4 py-3 text-right font-mono-tab text-muted">
                        {s.spins_count.toLocaleString()}
                      </td>
                      <td class="px-4 py-3 text-muted"><Time iso={s.created_at} /></td>
                      <td class="px-4 py-3">
                        {#if s.revoked_at}
                          <Badge tone="danger">Revoked</Badge>
                        {:else}
                          <Badge tone="accent">Active</Badge>
                        {/if}
                      </td>
                      <td class="px-4 py-3 text-right">
                        {#if s.revoked_at}
                          <span class="text-xs text-faint">—</span>
                        {:else}
                          <Button
                            variant="danger"
                            size="sm"
                            disabled={sharesBusy === s.id}
                            onclick={() => revokeShare(s)}
                          >
                            Revoke
                          </Button>
                        {/if}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          </Card>
        {/if}
      </section>
    {/if}
  {/if}
</main>
