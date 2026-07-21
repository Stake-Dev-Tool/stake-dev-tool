<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto, replaceState } from '$app/navigation';
  import {
    api,
    ApiError,
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
  import { roleTone, formatDate, formatExpiry, errorText } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import MathPushPanel from '$lib/components/MathPushPanel.svelte';
  import PlanBanner from '$lib/components/PlanBanner.svelte';

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
  let upgradedToast = $state('');

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
    if (page.url.searchParams.get('upgraded') !== '1') return;
    upgradedToast = 'Subscription active — welcome aboard.';
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
  <a href="/" class="mb-6 inline-flex items-center gap-1.5 text-sm text-muted transition hover:text-text">
    <span aria-hidden="true">←</span> Workspaces
  </a>

  {#if loading}
    <div class="flex items-center gap-3 py-16 text-muted"><span class="spinner"></span> Loading…</div>
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
      <a
        href={`/w/${slug}/billing`}
        class="ml-auto rounded-md border border-border bg-surface-2 px-3 py-1.5 text-sm text-muted transition hover:border-border-strong hover:text-text"
      >
        Billing
      </a>
    </header>

    <PlanBanner {slug} />

    {#if upgradedToast}
      <div
        class="fade-in mb-6 flex items-center gap-2 rounded-md border border-accent/40 bg-accent/10 px-4 py-3 text-sm text-accent"
      >
        <span aria-hidden="true">✓</span>
        {upgradedToast}
      </div>
    {/if}

    {#if actionError}
      <p class="mb-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
        {actionError}
      </p>
    {/if}

    <!-- Games -->
    <section class="mb-10">
      <div class="mb-3 flex items-center justify-between gap-3">
        <h2 class="text-sm font-semibold uppercase tracking-wide text-faint">
          Games · {games.length}
        </h2>
        {#if !showNewGame}
          <Button size="sm" onclick={() => (showNewGame = true)}>New game</Button>
        {/if}
      </div>

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

      <Card class="overflow-hidden">
        {#if gamesError}
          <div class="px-4 py-6">
            <p class="text-sm text-danger">{gamesError}</p>
            <Button variant="outline" size="sm" class="mt-3" onclick={loadGames}>Retry</Button>
          </div>
        {:else if games.length === 0}
          <div class="flex flex-col items-center gap-4 px-6 py-14 text-center">
            <span class="flex h-11 w-11 items-center justify-center rounded-xl bg-accent/10 text-accent">
              <svg
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <rect x="2" y="6" width="20" height="12" rx="2" />
                <path d="M6 12h4M8 10v4M15 12h.01M18 12h.01" />
              </svg>
            </span>
            <div>
              <h3 class="font-semibold">No games yet</h3>
              <p class="mx-auto mt-1 max-w-sm text-sm leading-relaxed text-muted">
                Push math straight from your browser with <span class="text-text">New game</span>
                above, or run <span class="font-mono-tab text-text">sdt push</span> from CI.
              </p>
            </div>
            <div class="w-full max-w-xs"><CopyField value="sdt push" /></div>
          </div>
        {:else}
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
        {/if}
      </Card>
    </section>

    <!-- Members -->
    <section class="mb-10">
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">
        Members · {members.length}
      </h2>
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
                    {#if canRemove(m)}
                      <Button
                        variant={self ? 'outline' : 'danger'}
                        size="sm"
                        disabled={busyUser === m.user_id}
                        onclick={() => removeMember(m)}
                      >
                        {self ? 'Leave' : 'Remove'}
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
      <section>
        <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Invites</h2>

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
  {/if}
</main>
