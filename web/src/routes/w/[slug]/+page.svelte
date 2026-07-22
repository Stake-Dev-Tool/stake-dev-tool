<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto, replaceState } from '$app/navigation';
  import {
    api,
    ApiError,
    isValidPlayDomain,
    type Role,
    type InviteRole,
    type Member,
    type Invite,
    type WorkspaceDetail,
    type CreatedInvite,
    type Game
  } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { invalidateBillingStatus } from '$lib/billing';
  import { toast } from '$lib/toasts.svelte';
  import { roleTone, formatDate, formatExpiry, errorText } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import MathPushPanel from '$lib/components/MathPushPanel.svelte';
  import PlanBanner from '$lib/components/PlanBanner.svelte';
  import Breadcrumbs from '$lib/components/Breadcrumbs.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import SectionHeader from '$lib/components/SectionHeader.svelte';
  import Tabs from '$lib/components/Tabs.svelte';

  let slug = $derived(page.params.slug ?? '');

  let detail = $state<WorkspaceDetail | null>(null);
  let invites = $state<Invite[]>([]);
  let games = $state<Game[]>([]);
  let gamesError = $state('');
  let showNewGame = $state(false);
  let loading = $state(true);
  let loadError = $state('');
  let actionError = $state('');
  let busyUser = $state<string | null>(null); // user_id currently mutating

  // Custom play domain (owner-only). `domainInput` is seeded from the loaded
  // detail; `domainError` surfaces inline validation + server errors. `appHost`
  // (this dashboard's own host) backs the DNS-setup instructions.
  let domainInput = $state('');
  let savingDomain = $state(false);
  let domainError = $state('');
  let appHost = $state('');

  // Client-side tabs (deep-linkable via #games / #settings). Games is primary.
  type WsTab = 'games' | 'settings';
  let activeTab = $state<WsTab>('games');
  function selectTab(id: string) {
    activeTab = id as WsTab;
    if (typeof history !== 'undefined') history.replaceState(history.state, '', `#${id}`);
  }

  // Derived permission context
  let myId = $derived(session.user?.id ?? '');
  let members = $derived(detail?.members ?? []);
  // Prefer the server's authoritative top-level role; fall back to matching our
  // user id against the member list.
  let myRole = $derived<Role | null>(
    detail?.role ?? members.find((m) => m.user_id === myId)?.role ?? null
  );
  let ownersCount = $derived(members.filter((m) => m.role === 'owner').length);
  let canManage = $derived(myRole === 'owner' || myRole === 'admin');
  let isOwner = $derived(myRole === 'owner');
  let selfMember = $derived(members.find((m) => m.user_id === myId) ?? null);
  // Sole owners can't leave (would orphan the workspace) — the danger zone says so.
  let canLeave = $derived(!!selfMember && !(selfMember.role === 'owner' && ownersCount <= 1));

  // Load on mount and whenever the :slug param changes (SvelteKit reuses this
  // component across /w/* navigations rather than remounting it).
  $effect(() => {
    void slug; // track the param
    load();
  });

  // Polar checkout success redirects here (the server's success_url is
  // /w/:slug?upgraded=1). Celebrate, drop any cached (pre-upgrade) billing status
  // so PlanBanner re-reads fresh, and strip the param. The redirect is a full page
  // load, so this one-shot onMount is the right hook.
  onMount(() => {
    appHost = location.hostname;
    // Deep-link to a tab via #hash.
    const h = location.hash.replace('#', '');
    if (h === 'settings' || h === 'games') activeTab = h;

    if (page.url.searchParams.get('upgraded') !== '1') return;
    toast.success('Subscription active — welcome aboard.');
    invalidateBillingStatus(slug);
    const url = new URL(page.url);
    url.searchParams.delete('upgraded');
    replaceState(url.pathname + url.search, {});
  });

  async function load() {
    loading = true;
    loadError = '';
    try {
      detail = await api.workspaces.get(slug);
      domainInput = detail.custom_play_domain ?? '';
      domainError = '';
      await loadGames();
      if (canManage) {
        await loadInvites();
      } else {
        invites = [];
      }
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  // A games-list failure must not take down the members/invites view, so it
  // carries its own inline error.
  async function loadGames() {
    gamesError = '';
    try {
      games = await api.games.list(slug);
    } catch (e) {
      games = [];
      gamesError = errorText(e);
    }
  }

  // A new game is created implicitly by its first commit; jump to the new
  // revision (its stats poll there).
  function onGamePushed(n: number, gameSlug: string) {
    showNewGame = false;
    if (n >= 1) void goto(`/w/${slug}/g/${gameSlug}/r/${n}`);
    else void goto(`/w/${slug}/g/${gameSlug}`);
  }

  async function loadInvites() {
    try {
      invites = await api.invites.list(slug);
    } catch {
      invites = [];
    }
  }

  // ---- Member role rules ------------------------------------------------
  function roleOptions(): Role[] {
    return myRole === 'owner' ? ['owner', 'admin', 'member'] : ['admin', 'member'];
  }

  function canEditRole(m: Member): boolean {
    if (!canManage) return false;
    // Owners are untouchable by anyone but themselves.
    if (m.role === 'owner' && m.user_id !== myId) return false;
    // The last owner cannot demote themselves (would orphan the workspace).
    if (m.user_id === myId && m.role === 'owner' && ownersCount <= 1) return false;
    // Admins may only manage members, not peers or owners.
    if (myRole === 'admin' && m.user_id !== myId && (m.role === 'admin' || m.role === 'owner'))
      return false;
    return true;
  }

  function canRemove(m: Member): boolean {
    if (m.user_id === myId) {
      // "Leave" — blocked only if you're the sole owner.
      return !(m.role === 'owner' && ownersCount <= 1);
    }
    if (!canManage) return false;
    if (myRole === 'owner') return m.role !== 'owner';
    if (myRole === 'admin') return m.role === 'member';
    return false;
  }

  async function changeRole(m: Member, ev: Event) {
    const next = (ev.currentTarget as HTMLSelectElement).value as Role;
    if (next === m.role) return;
    actionError = '';
    busyUser = m.user_id;
    try {
      await api.workspaces.setMemberRole(slug, m.user_id, next);
      await load();
    } catch (e) {
      actionError = errorText(e);
      await load(); // resync the select back to the server truth
    } finally {
      busyUser = null;
    }
  }

  async function removeMember(m: Member) {
    const isSelf = m.user_id === myId;
    const msg = isSelf
      ? `Leave "${detail?.workspace.name}"? You'll lose access unless re-invited.`
      : `Remove ${m.display_name} from "${detail?.workspace.name}"?`;
    if (!confirm(msg)) return;
    actionError = '';
    busyUser = m.user_id;
    try {
      await api.workspaces.removeMember(slug, m.user_id);
      if (isSelf) {
        await goto('/');
        return;
      }
      await load();
    } catch (e) {
      actionError = errorText(e);
    } finally {
      busyUser = null;
    }
  }

  // ---- Custom play domain (owner-only) ----------------------------------
  let domainCandidate = $derived(domainInput.trim().toLowerCase());
  // Live validity: empty is allowed (clears), else must pass the DNS rule.
  let domainValid = $derived(domainCandidate === '' || isValidPlayDomain(domainCandidate));
  let domainDirty = $derived(domainCandidate !== (detail?.custom_play_domain ?? ''));

  async function saveDomain(ev: SubmitEvent) {
    ev.preventDefault();
    if (savingDomain) return;
    domainError = '';
    const value = domainCandidate;
    if (value !== '' && !isValidPlayDomain(value)) {
      domainError =
        'Enter a valid domain like play.acme.com — at least two labels of letters, digits and hyphens.';
      return;
    }
    await persistDomain(value === '' ? null : value);
  }

  async function clearDomain() {
    if (savingDomain) return;
    domainError = '';
    await persistDomain(null);
  }

  async function persistDomain(domain: string | null) {
    savingDomain = true;
    try {
      const saved = await api.workspaces.setDomain(slug, domain);
      if (detail) detail = { ...detail, custom_play_domain: saved };
      domainInput = saved ?? '';
      toast.success(saved ? `Custom domain set to ${saved}` : 'Custom domain cleared.');
    } catch (e) {
      if (e instanceof ApiError && e.code === 'domain_taken') {
        domainError = 'That domain is already attached to another workspace.';
      } else if (e instanceof ApiError && e.code === 'invalid_domain') {
        domainError = e.message;
      } else {
        domainError = errorText(e);
      }
    } finally {
      savingDomain = false;
    }
  }

  // ---- Invites ----------------------------------------------------------
  let inviteRole = $state<InviteRole>('member');
  let inviteExpiry = $state('7'); // days, empty = never
  let inviteMaxUses = $state(''); // empty = unlimited
  let creatingInvite = $state(false);
  let inviteError = $state('');
  let revealed = $state<CreatedInvite | null>(null);

  async function createInvite(ev: SubmitEvent) {
    ev.preventDefault();
    if (creatingInvite) return;
    creatingInvite = true;
    inviteError = '';
    revealed = null;
    try {
      const expires = inviteExpiry.trim() === '' ? undefined : Number(inviteExpiry);
      const maxUses = inviteMaxUses.trim() === '' ? undefined : Number(inviteMaxUses);
      revealed = await api.invites.create(slug, {
        role: inviteRole,
        expires_in_days: Number.isFinite(expires as number) ? expires : undefined,
        max_uses: Number.isFinite(maxUses as number) ? maxUses : undefined
      });
      await loadInvites();
    } catch (e) {
      inviteError = errorText(e);
    } finally {
      creatingInvite = false;
    }
  }

  async function revoke(inv: Invite) {
    if (!confirm('Revoke this invite? Any unused link stops working immediately.')) return;
    try {
      await api.invites.revoke(slug, inv.id);
      await loadInvites();
    } catch (e) {
      actionError = errorText(e);
    }
  }

  function inviteStatus(inv: Invite): { label: string; tone: 'neutral' | 'accent' | 'warn' | 'danger' } {
    if (inv.revoked_at) return { label: 'Revoked', tone: 'danger' };
    if (inv.expires_at && new Date(inv.expires_at).getTime() < Date.now())
      return { label: 'Expired', tone: 'warn' };
    if (inv.max_uses != null && inv.uses >= inv.max_uses) return { label: 'Used up', tone: 'warn' };
    return { label: 'Active', tone: 'accent' };
  }
</script>

<svelte:head><title>{detail?.workspace.name ?? slug} · Stake Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-5xl px-6 py-10">
  <Breadcrumbs
    items={[{ label: 'Workspaces', href: '/' }, { label: detail?.workspace.name ?? slug }]}
  />

  {#if loading}
    <Card class="p-6"><Skeleton /></Card>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={load}>Retry</Button>
    </Card>
  {:else if detail}
    <header class="mb-6 flex flex-wrap items-center gap-3">
      <h1 class="text-2xl font-semibold tracking-tight">{detail.workspace.name}</h1>
      <span class="font-mono-tab text-sm text-muted">{detail.workspace.slug}</span>
      {#if myRole}<Badge tone={roleTone(myRole)}>{myRole}</Badge>{/if}
    </header>

    <PlanBanner {slug} />

    <Tabs
      class="mb-6"
      tabs={[
        { id: 'games', label: 'Games', badge: games.length },
        { id: 'settings', label: 'Settings' }
      ]}
      active={activeTab}
      onselect={selectTab}
    />

    {#if activeTab === 'games'}
      <!-- Games — primary, dominant -->
      <section>
        <SectionHeader title={`Games · ${games.length}`}>
          {#snippet action()}
            {#if !showNewGame}
              <Button size="sm" onclick={() => (showNewGame = true)}>New game</Button>
            {/if}
          {/snippet}
        </SectionHeader>

        {#if showNewGame}
          <div class="mb-4">
            <MathPushPanel
              {slug}
              game={null}
              parentNumber={null}
              ondone={(n, gameSlug) => onGamePushed(n, gameSlug)}
              oncancel={() => (showNewGame = false)}
            />
          </div>
        {/if}

        {#if gamesError}
          <Card class="p-6">
            <p class="text-sm text-danger">{gamesError}</p>
            <Button variant="outline" size="sm" class="mt-3" onclick={loadGames}>Retry</Button>
          </Card>
        {:else if games.length === 0}
          <EmptyState title="No games yet">
            Push math straight from your browser with <span class="text-text">New game</span> above,
            or run <span class="font-mono-tab text-text">sdt push</span> from CI.
            {#snippet cta()}
              <div class="w-full max-w-xs"><CopyField value="sdt push" /></div>
            {/snippet}
          </EmptyState>
        {:else}
          <Card class="overflow-hidden">
            {#each games as g (g.id || g.slug)}
              <a
                href={`/w/${slug}/g/${g.slug}`}
                class="flex items-center justify-between gap-4 border-b border-border/60 px-4 py-3.5 transition last:border-0 hover:bg-surface-2"
              >
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="truncate font-medium">{g.name}</span>
                    {#if g.head_number != null}
                      <Badge tone="accent">rev {g.head_number}</Badge>
                    {:else}
                      <Badge>no revisions</Badge>
                    {/if}
                  </div>
                  <div class="mt-0.5 truncate font-mono-tab text-xs text-muted">{g.slug}</div>
                </div>
                <div class="shrink-0 text-xs text-faint">
                  {g.revisions_count}
                  {g.revisions_count === 1 ? 'revision' : 'revisions'}
                </div>
              </a>
            {/each}
          </Card>
        {/if}
      </section>
    {:else}
      <!-- Settings — members, invites, billing, danger zone -->
      {#if actionError}
        <p class="mb-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
          {actionError}
        </p>
      {/if}

      <!-- Members -->
      <section class="mb-10">
        <SectionHeader title={`Members · ${members.length}`} />
        <Card class="overflow-hidden">
        <div class="overflow-x-auto">
          <table class="w-full min-w-[34rem] text-sm">
            <thead>
              <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                <th class="px-4 py-3 font-medium">Member</th>
                <th class="px-4 py-3 font-medium">Role</th>
                <th class="px-4 py-3 font-medium text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {#each members as m (m.user_id)}
                {@const self = m.user_id === myId}
                <tr class="border-b border-border/60 last:border-0">
                  <td class="px-4 py-3">
                    <div class="flex items-center gap-2">
                      <span class="font-medium">{m.display_name || '—'}</span>
                      {#if self}<Badge>you</Badge>{/if}
                    </div>
                    {#if m.email}<div class="mt-0.5 text-xs text-faint">{m.email}</div>{/if}
                  </td>
                  <td class="px-4 py-3">
                    {#if canEditRole(m)}
                      <select
                        value={m.role}
                        disabled={busyUser === m.user_id}
                        onchange={(e) => changeRole(m, e)}
                        class="h-8 rounded-md border border-border bg-surface-2 px-2 text-sm text-text outline-none transition focus:border-accent/60 focus:ring-2 focus:ring-accent/25 disabled:opacity-50"
                      >
                        {#each roleOptions() as r (r)}
                          <option value={r}>{r}</option>
                        {/each}
                      </select>
                    {:else}
                      <Badge tone={roleTone(m.role)}>{m.role}</Badge>
                    {/if}
                  </td>
                  <td class="px-4 py-3 text-right">
                    {#if !self && canRemove(m)}
                      <Button
                        variant="danger"
                        size="sm"
                        disabled={busyUser === m.user_id}
                        onclick={() => removeMember(m)}
                      >
                        Remove
                      </Button>
                    {:else}
                      <span class="text-xs text-faint">—</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </Card>
    </section>

    <!-- Invites (owner/admin only) -->
    {#if canManage}
      <section class="mb-10">
        <SectionHeader title="Invites" />

        <Card class="mb-4 p-6">
          <form class="flex flex-col gap-4" onsubmit={createInvite}>
            <div class="grid gap-4 sm:grid-cols-3">
              <label class="flex flex-col gap-1.5">
                <span class="text-sm font-medium text-muted">Role</span>
                <select
                  bind:value={inviteRole}
                  class="h-9 rounded-md border border-border bg-surface-2 px-3 text-sm text-text outline-none transition focus:border-accent/60 focus:ring-2 focus:ring-accent/25"
                >
                  <option value="member">member</option>
                  <option value="admin">admin</option>
                </select>
              </label>
              <Input
                id="inv-expiry"
                label="Expires in (days)"
                type="number"
                min="1"
                bind:value={inviteExpiry}
                placeholder="never"
                hint="Blank = never expires"
              />
              <Input
                id="inv-max"
                label="Max uses"
                type="number"
                min="1"
                bind:value={inviteMaxUses}
                placeholder="unlimited"
                hint="Blank = unlimited"
              />
            </div>

            {#if inviteError}
              <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
                {inviteError}
              </p>
            {/if}

            <div><Button type="submit" loading={creatingInvite}>Create invite link</Button></div>
          </form>

          {#if revealed}
            <div class="fade-in mt-5 rounded-md border border-accent/30 bg-accent/5 p-4">
              <div class="mb-2 flex items-center gap-2">
                <Badge tone="accent">New invite</Badge>
                <span class="text-xs text-warn">Copy it now — you won't see this link again.</span>
              </div>
              <CopyField label="Invite link" value={revealed.invite_url || revealed.token} />
              {#if revealed.invite_url && revealed.token}
                <div class="mt-3"><CopyField label="Token" value={revealed.token} /></div>
              {/if}
            </div>
          {/if}
        </Card>

        <Card class="overflow-hidden">
          {#if invites.length === 0}
            <p class="px-4 py-8 text-center text-sm text-muted">No invites yet.</p>
          {:else}
            <div class="overflow-x-auto">
              <table class="w-full min-w-[40rem] text-sm">
                <thead>
                  <tr class="border-b border-border text-left text-xs uppercase tracking-wide text-faint">
                    <th class="px-4 py-3 font-medium">Role</th>
                    <th class="px-4 py-3 font-medium">Status</th>
                    <th class="px-4 py-3 font-medium">Uses</th>
                    <th class="px-4 py-3 font-medium">Expires</th>
                    <th class="px-4 py-3 font-medium">Created</th>
                    <th class="px-4 py-3 font-medium text-right">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {#each invites as inv (inv.id)}
                    {@const status = inviteStatus(inv)}
                    {@const revocable = !inv.revoked_at}
                    <tr class="border-b border-border/60 last:border-0">
                      <td class="px-4 py-3"><Badge tone={roleTone(inv.role)}>{inv.role}</Badge></td>
                      <td class="px-4 py-3"><Badge tone={status.tone}>{status.label}</Badge></td>
                      <td class="px-4 py-3 font-mono-tab text-muted">
                        {inv.uses}{inv.max_uses != null ? ` / ${inv.max_uses}` : ''}
                      </td>
                      <td class="px-4 py-3 text-muted">{formatExpiry(inv.expires_at)}</td>
                      <td class="px-4 py-3 text-muted">{formatDate(inv.created_at)}</td>
                      <td class="px-4 py-3 text-right">
                        {#if revocable}
                          <Button variant="danger" size="sm" onclick={() => revoke(inv)}>Revoke</Button>
                        {:else}
                          <span class="text-xs text-faint">—</span>
                        {/if}
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

      <!-- Billing -->
      <section class="mb-10">
        <SectionHeader title="Billing" />
        <Card class="flex flex-wrap items-center justify-between gap-3 p-5">
          <p class="text-sm text-muted">Manage your plan, usage and invoices.</p>
          <Button href={`/w/${slug}/billing`} variant="secondary" size="sm">Open billing</Button>
        </Card>
      </section>

      <!-- Custom play domain (owner-only) -->
      {#if isOwner}
        <section class="mb-10">
          <SectionHeader title="Custom play domain" />
          <Card class="p-6">
            <p class="mb-4 text-sm text-muted">
              Serve this workspace's share links from a domain you own — e.g.
              <span class="font-mono-tab text-text">play.acme.com</span>. Once set, every share link is
              reachable at
              <span class="font-mono-tab text-text"
                >&lt;slug&gt;.{domainCandidate || 'play.acme.com'}</span
              >, with its TLS certificate issued automatically on the first visit.
            </p>

            <form class="flex flex-col gap-3 sm:flex-row sm:items-end" onsubmit={saveDomain}>
              <div class="flex-1">
                <Input
                  id="custom-domain"
                  label="Domain"
                  mono
                  bind:value={domainInput}
                  placeholder="play.acme.com"
                  autocomplete="off"
                  spellcheck={false}
                  error={domainError}
                  hint="Lowercase, at least two labels. Leave blank and Save to remove."
                />
              </div>
              <div class="flex gap-2">
                <Button type="submit" loading={savingDomain} disabled={!domainValid || !domainDirty}>
                  Save
                </Button>
                {#if detail.custom_play_domain}
                  <Button
                    type="button"
                    variant="outline"
                    disabled={savingDomain}
                    onclick={clearDomain}
                  >
                    Clear
                  </Button>
                {/if}
              </div>
            </form>

            {#if domainCandidate !== '' && !domainValid && !domainError}
              <p class="mt-2 text-xs text-warn">
                That doesn't look like a valid domain — need at least two labels of letters, digits and
                hyphens (e.g. <span class="font-mono-tab">play.acme.com</span>).
              </p>
            {/if}

            {#if detail.custom_play_domain}
              <div class="fade-in mt-5 rounded-md border border-accent/30 bg-accent/5 p-4">
                <div class="mb-2 flex items-center gap-2">
                  <Badge tone="accent">Active</Badge>
                  <span class="font-mono-tab text-sm text-text">{detail.custom_play_domain}</span>
                </div>
                <p class="text-sm text-muted">
                  Your share links now use
                  <span class="font-mono-tab text-text"
                    >&lt;slug&gt;.{detail.custom_play_domain}</span
                  >.
                </p>

                <div class="mt-4 border-t border-border/60 pt-4">
                  <div class="text-xs font-medium uppercase tracking-wide text-faint">DNS setup</div>
                  <p class="mt-2 text-sm text-muted">
                    Add a <span class="text-text">wildcard</span> record so every share subdomain
                    resolves here, in <span class="text-text">DNS-only</span> mode (grey cloud on
                    Cloudflare):
                  </p>
                  <div class="mt-3 grid gap-2">
                    <CopyField label="Wildcard record" value={`*.${detail.custom_play_domain}`} />
                    {#if appHost}
                      <CopyField label="Points at (CNAME target)" value={appHost} />
                    {/if}
                  </div>
                  <p class="mt-3 text-xs text-faint">
                    Point it at this dashboard's host: a <span class="text-text">CNAME</span> to
                    {appHost || 'the app host'}, or an <span class="text-text">A</span> record to that
                    host's IP. The first visit to a share link issues its certificate automatically —
                    that request can take ~30 s.
                  </p>
                </div>
              </div>
            {/if}
          </Card>
        </section>
      {/if}

      <!-- Danger zone -->
      <section>
        <SectionHeader title="Danger zone" />
        <Card class="border-danger/30 p-5">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <div class="min-w-0">
              <div class="text-sm font-medium text-text">Leave this workspace</div>
              <p class="mt-0.5 text-sm text-muted">
                {canLeave
                  ? "You'll lose access to its games and math until you're re-invited."
                  : "You're the only owner — add another owner before you can leave."}
              </p>
            </div>
            <Button
              variant="danger"
              size="sm"
              disabled={!canLeave || busyUser === myId}
              onclick={() => selfMember && removeMember(selfMember)}
            >
              Leave workspace
            </Button>
          </div>
        </Card>
      </section>
    {/if}
  {/if}
</main>
