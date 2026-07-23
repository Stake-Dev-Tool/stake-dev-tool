<script lang="ts">
  import { page } from '$app/state';
  import { goto, replaceState } from '$app/navigation';
  import { api, type BillingStatus, type BillingInterval, type Role } from '$lib/api';
  import {
    billingStatus,
    setBillingStatus,
    invalidateBillingStatus,
    planLabel,
    statusLabel,
    intervalLabel,
    meter,
    meterFill,
    clampSeats,
    seatEntitlements,
    priceSummary,
    FREE_PLAN,
    PER_SEAT,
    SEATS_MIN,
    SEATS_MAX,
    SEAT_FIRST_EUR,
    SEAT_ADDITIONAL_EUR,
    clampStorageUnits,
    storageMonthlyEur,
    STORAGE_UNIT_GIB,
    STORAGE_UNITS_MIN,
    STORAGE_UNITS_MAX
  } from '$lib/billing';
  import { humanSize, formatDate, errorText } from '$lib/format';
  import { toast } from '$lib/toasts.svelte';
  import { workspaceName } from '$lib/workspaces.svelte';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Badge from '$lib/components/Badge.svelte';
  import Breadcrumbs from '$lib/components/Breadcrumbs.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';

  let slug = $derived(page.params.slug ?? '');

  let status = $state<BillingStatus | null>(null);
  let role = $state<Role | null>(null);
  let wsName = $state('');
  let loading = $state(true);
  let loadError = $state('');

  // Set from ?new=1 (a just-created, still-Free workspace): show the focused
  // "pick a plan to activate" header with a Pay-later escape hatch.
  let activating = $state(false);

  // Subscribe (free workspace → Stripe checkout).
  let checkoutError = $state('');
  let checkoutBusy = $state(false);

  // Seat stepper (1..100) + a monthly/yearly toggle, shared by the Subscribe card
  // (free) and the Change-seats control (paid).
  let seats = $state(1);
  let interval = $state<BillingInterval>('monthly');
  let seatsSeeded = false;
  const INTERVALS: BillingInterval[] = ['monthly', 'yearly'];

  // Storage add-on the Subscribe card bundles into the SAME checkout (0 = none).
  let checkoutStorage = $state(0);

  // Change seats on an existing subscription (proration, in place).
  let seatsBusy = $state(false);
  let seatsError = $state('');

  // Storage add-on for a subscribed workspace (additive "add more" → checkout).
  let storageUnits = $state(1);
  let storageBusy = $state(false);
  let storageError = $state('');

  // Stripe Customer Portal (manage payment method, invoices, cancel).
  let portalBusy = $state(false);
  let portalError = $state('');

  let isOwner = $derived(role === 'owner');
  let isFree = $derived(status?.plan === 'free');
  let isPaid = $derived(status?.plan === 'paid');

  let currentSeats = $derived(status?.seats ?? 0);
  let memberCount = $derived(status?.usage.members ?? 0);
  let belowMembers = $derived(seats < memberCount);
  let seatsChanged = $derived(seats !== currentSeats);
  let paidStorageUnits = $derived(
    status ? Math.round((status.extra_storage_gib || 0) / STORAGE_UNIT_GIB) : 0
  );
  let paidInterval = $derived<BillingInterval>(status?.interval ?? 'monthly');

  // Usage meters — only meaningful (shown with caps) once a plan is active.
  let meters = $derived(
    status
      ? [
          {
            label: 'Members',
            usage: status.usage.members,
            limit: status.limits.max_members,
            fmt: (n: number) => n.toLocaleString()
          },
          {
            label: 'Storage',
            usage: status.usage.storage_bytes,
            limit: status.limits.max_storage_bytes,
            fmt: (n: number) => humanSize(n)
          },
          {
            label: 'Active share links',
            usage: status.usage.active_share_links,
            limit: status.limits.max_active_share_links,
            fmt: (n: number) => n.toLocaleString()
          }
        ]
      : []
  );

  // When the current period date means something, phrase it for the plan state.
  let periodLine = $derived.by(() => {
    if (!status || !status.current_period_end) return '';
    const d = formatDate(status.current_period_end);
    // A scheduled cancellation keeps the status "active" but stops the renewal.
    if (status.status === 'canceled' || status.cancel_at_period_end) return `Access until ${d}`;
    return `Renews ${d}`;
  });

  function plural(n: number, one: string, many = one + 's'): string {
    return `${n.toLocaleString()} ${n === 1 ? one : many}`;
  }

  function payLater() {
    void goto(`/w/${slug}`);
  }

  $effect(() => {
    void slug;
    load();
  });

  async function load() {
    loading = true;
    loadError = '';
    try {
      const [detail, cached] = await Promise.all([
        api.workspaces.get(slug).catch(() => null),
        billingStatus(slug)
      ]);
      role = detail?.role ?? null;
      wsName = detail?.workspace.name ?? '';
      status = cached;
      seedSeats();

      // Stripe success redirect (?upgraded=1): celebrate, refetch fresh (the
      // cache may predate the subscription), and strip the param. Read here —
      // after the await — so this effect never subscribes to page.url.
      if (page.url.searchParams.get('upgraded') === '1') {
        toast.success('Subscription active — welcome aboard.');
        try {
          const fresh = await api.billing.status(slug);
          setBillingStatus(slug, fresh);
          status = fresh;
          seatsSeeded = false;
          seedSeats();
        } catch {
          // Keep the cached status; the toast still stands.
        }
        const url = new URL(page.url);
        url.searchParams.delete('upgraded');
        replaceState(url.pathname + url.search, {});
      }

      // Fresh-workspace activation (?new=1): show the focused header, then strip
      // the param so a refresh doesn't re-trigger it. Read after the awaits so
      // this effect never subscribes to page.url.
      if (page.url.searchParams.get('new') === '1') {
        activating = true;
        const url = new URL(page.url);
        url.searchParams.delete('new');
        replaceState(url.pathname + url.search, {});
      }
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  // Seed the seat stepper once: a paid workspace's current seat count, else the
  // current member count (so the default at least covers today's team). Clamped ≥ 1.
  function seedSeats() {
    if (seatsSeeded || !status) return;
    const base = status.seats ?? status.usage.members ?? 1;
    seats = clampSeats(base);
    if (status.interval) interval = status.interval;
    seatsSeeded = true;
  }

  function stepSeats(delta: number) {
    seats = clampSeats(seats + delta);
  }

  function clampCheckoutStorage(n: number): number {
    if (!Number.isFinite(n)) return 0;
    return Math.min(STORAGE_UNITS_MAX, Math.max(0, Math.round(n)));
  }
  function stepCheckoutStorage(delta: number) {
    checkoutStorage = clampCheckoutStorage(checkoutStorage + delta);
  }

  // Free workspace: one checkout for seats + (optional) storage add-on.
  async function subscribe() {
    if (!isOwner || checkoutBusy) return;
    checkoutBusy = true;
    checkoutError = '';
    try {
      const url = await api.billing.checkout(
        slug,
        interval,
        clampSeats(seats),
        clampCheckoutStorage(checkoutStorage)
      );
      if (url) {
        window.location.href = url; // full navigation to the hosted checkout
        return; // leave the button busy; the page is unloading
      }
      checkoutError = 'Checkout is unavailable right now. Please try again.';
    } catch (e) {
      checkoutError = errorText(e);
    }
    checkoutBusy = false;
  }

  // Paid workspace: change the seat count in place (Stripe prorates it).
  async function applySeats() {
    if (!isOwner || seatsBusy) return;
    const n = clampSeats(seats);
    if (n === currentSeats || belowMembers) return;
    seatsBusy = true;
    seatsError = '';
    try {
      const fresh = await api.billing.updateSeats(slug, n);
      // Reflect it everywhere: update this page, then drop the shared cache so the
      // PlanBanner (and other mounts) refetch the new seat-derived limits.
      status = fresh;
      invalidateBillingStatus(slug);
      seatsSeeded = false;
      seedSeats();
      toast.success('Seats updated — Stripe will prorate the difference on your next invoice.');
    } catch (e) {
      seatsError = errorText(e);
    }
    seatsBusy = false;
  }

  function stepStorage(delta: number) {
    storageUnits = clampStorageUnits(storageUnits + delta);
  }

  // Paid workspace: add more storage (a separate add-on subscription → checkout).
  async function buyStorage() {
    if (!isOwner || storageBusy) return;
    const units = clampStorageUnits(storageUnits);
    storageBusy = true;
    storageError = '';
    try {
      const url = await api.billing.buyStorage(slug, units);
      if (url) {
        window.location.href = url; // full navigation to the hosted checkout
        return; // leave the button busy; the page is unloading
      }
      storageError = 'Checkout is unavailable right now. Please try again.';
    } catch (e) {
      storageError = errorText(e);
    }
    storageBusy = false;
  }

  // Paid workspace: open the Stripe Customer Portal (full navigation to Stripe).
  async function openPortal() {
    if (!isOwner || portalBusy) return;
    portalBusy = true;
    portalError = '';
    try {
      const url = await api.billing.portal(slug);
      if (url) {
        window.location.href = url; // full navigation to the hosted portal
        return; // leave the button busy; the page is unloading
      }
      portalError = 'Billing management is unavailable right now. Please try again.';
    } catch (e) {
      portalError = errorText(e);
    }
    portalBusy = false;
  }
</script>

<svelte:head><title>Billing · {wsName || workspaceName(slug)} · Stake Dev Tool Cloud</title></svelte:head>

<!-- ── Reusable snippets ─────────────────────────────────────────────────── -->

{#snippet stepperControl(
  display: string,
  dec: () => void,
  inc: () => void,
  decDisabled: boolean,
  incDisabled: boolean,
  decAria: string,
  incAria: string,
  minW = '4rem'
)}
  <div class="inline-flex items-center rounded-md border border-border">
    <button
      type="button"
      class="px-3 py-1.5 text-lg leading-none text-muted transition hover:text-text disabled:opacity-40"
      aria-label={decAria}
      disabled={decDisabled}
      onclick={dec}
    >
      −
    </button>
    <span
      class="px-3 text-center font-mono-tab text-sm font-medium text-text"
      style="min-width: {minW}"
    >
      {display}
    </span>
    <button
      type="button"
      class="px-3 py-1.5 text-lg leading-none text-muted transition hover:text-text disabled:opacity-40"
      aria-label={incAria}
      disabled={incDisabled}
      onclick={inc}
    >
      +
    </button>
  </div>
{/snippet}

<!-- Live "what you get" for a chosen seat count. -->
{#snippet entitlements(n: number)}
  {@const e = seatEntitlements(n)}
  <div>
    <div class="mb-2 text-sm font-medium text-text">What you get with {plural(n, 'seat')}</div>
    <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
      {#each [{ v: e.members, u: n === 1 ? 'member' : 'members' }, { v: `${e.storageGib} GiB`, u: 'storage' }, { v: e.shareLinks, u: 'share links' }] as cell (cell.u)}
        <div class="rounded-lg border border-border bg-surface-2/40 px-3 py-2.5">
          <div class="font-mono-tab text-lg font-semibold leading-tight text-text">{cell.v}</div>
          <div class="text-xs text-muted">{cell.u}</div>
        </div>
      {/each}
    </div>
    <p class="mt-2 text-xs text-faint">
      Each seat = {PER_SEAT.members} team member + {PER_SEAT.storageGib} GiB storage +
      {PER_SEAT.shareLinks} share links.
    </p>
  </div>
{/snippet}

<!-- Live combined price summary for `seatsN` seats + `storageN` storage units. -->
{#snippet priceBox(seatsN: number, storageN: number, iv: BillingInterval)}
  {@const p = priceSummary(seatsN, storageN)}
  <div class="rounded-lg border border-border bg-surface-2/40 p-4 text-sm">
    <div class="flex items-baseline justify-between gap-3">
      <span class="text-muted">Seats ({seatsN})</span>
      <span class="font-mono-tab text-text">
        {iv === 'yearly' ? `€${p.seatYearly} / yr` : `€${p.seatMonthly} / mo`}
      </span>
    </div>
    {#if storageN > 0}
      <div class="mt-1 flex items-baseline justify-between gap-3">
        <span class="text-muted">Storage add-on (+{storageN * STORAGE_UNIT_GIB} GiB)</span>
        <span class="font-mono-tab text-text">€{p.storageMonthly} / mo</span>
      </div>
    {/if}
    <div
      class="mt-2 flex items-baseline justify-between gap-3 border-t border-border pt-2 font-semibold"
    >
      <span class="text-text">Total</span>
      <span class="font-mono-tab text-text">
        {#if iv === 'yearly'}
          €{p.seatYearly} / yr{#if storageN > 0}<span class="text-muted"> + €{p.storageMonthly} / mo</span>{/if}
        {:else}
          €{p.monthlyTotal} / mo
        {/if}
      </span>
    </div>
    {#if iv === 'yearly'}
      <div class="mt-1 text-right text-xs text-accent">2 months free — save €{p.yearlySaving} / yr</div>
    {/if}
  </div>
{/snippet}

<main class="mx-auto w-full max-w-4xl px-6 py-10">
  <Breadcrumbs
    items={[{ label: wsName || workspaceName(slug), href: `/w/${slug}` }, { label: 'Billing' }]}
  />

  {#if activating}
    <div class="mb-8">
      <h1 class="text-2xl font-semibold tracking-tight">
        {wsName || workspaceName(slug)} is ready
      </h1>
      <p class="mt-2 max-w-prose text-sm leading-relaxed text-muted">
        You're on the Free plan — create games, push math and share them right away, no card
        required. Upgrade whenever you need teammates, full revision history, or share links
        that outlive {FREE_PLAN.shareLinkDays} days.
      </p>
      <Button size="sm" class="mt-4" onclick={payLater}>Start building →</Button>
    </div>
  {:else}
    <h1 class="mb-8 text-2xl font-semibold tracking-tight">Billing</h1>
  {/if}

  {#if loading}
    <Card class="p-6"><Skeleton /></Card>
  {:else if loadError}
    <Card class="p-6">
      <p class="text-sm text-danger">{loadError}</p>
      <Button variant="outline" size="sm" class="mt-4" onclick={load}>Retry</Button>
    </Card>
  {:else if status && !status.enabled}
    <!-- Self-hosted: billing is off and every limit is unlimited. -->
    <Card class="p-6">
      <h2 class="text-base font-semibold">Billing isn't enabled on this instance</h2>
      <p class="mt-1.5 max-w-prose text-sm leading-relaxed text-muted">
        Self-hosted instances run with every feature unlimited — no plans, no quotas, nothing to
        manage here.
      </p>
    </Card>
  {:else if status}
    <!-- Current plan -->
    <section class="mb-8">
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Current plan</h2>
      <Card class="p-6">
        <div class="flex flex-wrap items-center gap-3">
          <span class="text-lg font-semibold">{planLabel(status.plan)}</span>
          {#if isPaid && status.seats != null}
            <span class="text-sm text-muted">· {plural(status.seats, 'seat')}</span>
          {/if}
          {#if status.status}
            <Badge tone={status.status === 'past_due' ? 'warn' : 'accent'}>
              {statusLabel(status.status)}
            </Badge>
          {:else if isFree}
            <Badge tone="accent">Active</Badge>
          {/if}
          {#if status.interval}
            <span class="text-sm text-muted">· {intervalLabel(status.interval)}</span>
          {/if}
          {#if isPaid && status.extra_storage_gib > 0}
            <span class="text-sm text-muted">· +{status.extra_storage_gib} GiB storage</span>
          {/if}
        </div>
        {#if periodLine}
          <p class="mt-1.5 text-sm text-muted">{periodLine}</p>
        {/if}
        {#if isPaid && status.cancel_at_period_end && status.current_period_end}
          <p class="mt-3 rounded-md border border-warn/30 bg-warn/10 px-3 py-2 text-sm text-warn">
            Your plan ends on {formatDate(status.current_period_end)} — resubscribe anytime.
          </p>
        {/if}
      </Card>
    </section>

    <!-- Usage -->
    <section class="mb-8">
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Usage</h2>
      <Card class="flex flex-col gap-5 p-6">
        {#each meters as m (m.label)}
          {#if !(isFree && m.label === 'Members')}
            {@const mt = meter(m.usage, m.limit)}
            <div>
              <div class="mb-1.5 flex items-baseline justify-between gap-3 text-sm">
                <span class="text-muted">{m.label}</span>
                <span class="font-mono-tab text-text">
                  {m.fmt(m.usage)}
                  <span class="text-faint">/ {mt.unlimited ? '∞' : m.fmt(m.limit ?? 0)}</span>
                </span>
              </div>
              <div class="h-2 w-full overflow-hidden rounded-full bg-surface-2">
                {#if !mt.unlimited}
                  <div
                    class="h-full rounded-full {meterFill(mt.tone)} transition-all"
                    style="width: {mt.pct}%"
                  ></div>
                {/if}
              </div>
            </div>
          {/if}
        {/each}
        {#if isFree}
          <!-- The Free rules, stated as plain facts rather than alarming meters:
               a fresh solo workspace is at "capacity" on members by design. -->
          <ul class="flex flex-col gap-2 border-t border-border/60 pt-4 text-sm">
            <li class="flex items-baseline justify-between gap-3">
              <span class="text-muted">Members</span>
              <span class="text-text">
                Just you <span class="text-faint">— seats add teammates</span>
              </span>
            </li>
            <li class="flex items-baseline justify-between gap-3">
              <span class="text-muted">Revisions per game</span>
              <span class="text-text">
                Latest only <span class="text-faint">— each push replaces the previous</span>
              </span>
            </li>
            <li class="flex items-baseline justify-between gap-3">
              <span class="text-muted">Share link lifetime</span>
              <span class="text-text">
                Up to {status.limits.max_share_link_days ?? FREE_PLAN.shareLinkDays} days
              </span>
            </li>
            <li class="flex items-baseline justify-between gap-3">
              <span class="text-muted">Play sessions</span>
              <span class="text-text">Unlimited</span>
            </li>
          </ul>
        {/if}
      </Card>
    </section>

    {#if isPaid}
      <!-- Manage an active subscription: seats (prorated in place) + storage add-on. -->
      <section>
        <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">
          Your subscription
        </h2>

        {#if !isOwner}
          <p class="mb-4 rounded-md border border-border bg-surface-2/60 px-3 py-2 text-sm text-muted">
            Only the workspace owner can manage billing.
          </p>
        {/if}

        <Card class="flex flex-col gap-6 p-6">
          <!-- Seats -->
          <div class="flex flex-col gap-4">
            <div>
              <div class="text-base font-semibold">Seats</div>
              <p class="mt-1 max-w-prose text-sm text-muted">
                Changes are prorated by Stripe — you're only billed the difference on your next
                invoice.
              </p>
            </div>

            <div class="flex flex-wrap items-center gap-4">
              {@render stepperControl(
                String(seats),
                () => stepSeats(-1),
                () => stepSeats(1),
                !isOwner || seats <= SEATS_MIN,
                !isOwner || seats >= SEATS_MAX,
                'Fewer seats',
                'More seats'
              )}
              <span class="text-sm text-muted">
                currently {plural(currentSeats, 'seat')}
                {#if seatsChanged}<span class="text-faint">→ {seats}</span>{/if}
              </span>
            </div>

            {@render entitlements(seats)}

            {#if belowMembers}
              <p class="rounded-md border border-warn/30 bg-warn/10 px-3 py-2 text-sm text-warn">
                Keep at least {plural(memberCount, 'seat')} — this workspace has {plural(
                  memberCount,
                  'member'
                )}. Remove members first to go lower.
              </p>
            {/if}
            {#if seatsError}
              <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
                {seatsError}
              </p>
            {/if}

            <Button
              class="w-fit"
              loading={seatsBusy}
              disabled={!isOwner || seatsBusy || !seatsChanged || belowMembers}
              onclick={applySeats}
            >
              Update seats
            </Button>
          </div>

          <hr class="border-border" />

          <!-- Storage add-on -->
          <div class="flex flex-col gap-4">
            <div>
              <div class="text-base font-semibold">Storage add-on</div>
              <p class="mt-1 max-w-prose text-sm text-muted">
                Add storage in {STORAGE_UNIT_GIB} GiB units for €1/mo each. It stacks on top of your
                plan's storage cap.
              </p>
            </div>

            {#if status.extra_storage_gib > 0}
              <p class="text-sm">
                <span class="font-medium text-text">Current add-on:</span>
                <span class="font-mono-tab">{status.extra_storage_gib} GiB</span>
                <span class="text-faint">· €{status.extra_storage_gib / STORAGE_UNIT_GIB} / mo</span>
              </p>
            {/if}

            {#if storageError}
              <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
                {storageError}
              </p>
            {/if}

            <div class="flex flex-wrap items-center gap-4">
              {@render stepperControl(
                `+${storageUnits * STORAGE_UNIT_GIB} GiB`,
                () => stepStorage(-1),
                () => stepStorage(1),
                !isOwner || storageUnits <= STORAGE_UNITS_MIN,
                !isOwner || storageUnits >= STORAGE_UNITS_MAX,
                'Fewer units',
                'More units',
                '7rem'
              )}
              <span class="text-sm text-muted">
                <span class="font-semibold text-text">€{storageMonthlyEur(storageUnits)} / mo</span>
              </span>
              <Button loading={storageBusy} disabled={!isOwner || storageBusy} onclick={buyStorage}>
                {status.extra_storage_gib > 0 ? 'Add more storage' : 'Add storage'}
              </Button>
            </div>
          </div>

          <hr class="border-border" />

          <!-- Combined summary of the projected subscription -->
          {@render priceBox(seats, paidStorageUnits, paidInterval)}

          <hr class="border-border" />

          <!-- Manage billing via Stripe's Customer Portal (invoices, payment
               method, cancel) — a quiet secondary action, owner-only. -->
          <div class="flex flex-col gap-3">
            <div>
              <div class="text-base font-semibold">Manage billing</div>
              <p class="mt-1 max-w-prose text-sm text-muted">
                Invoices, payment method, or cancel — handled securely by Stripe.
              </p>
            </div>

            {#if portalError}
              <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
                {portalError}
              </p>
            {/if}

            <Button
              variant="outline"
              class="w-fit"
              loading={portalBusy}
              disabled={!isOwner || portalBusy}
              onclick={openPortal}
            >
              Manage billing
            </Button>
          </div>
        </Card>

        <p class="mt-4 text-xs text-faint">
          Prices exclude tax — VAT, when applicable, is added at checkout based on your country.
          Payments are processed securely by Stripe as merchant of record.
        </p>
      </section>
    {:else}
      <!-- Upgrade: seats + interval + optional storage, all in one checkout. -->
      <section>
        <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Upgrade</h2>

        {#if !isOwner}
          <p class="mb-4 rounded-md border border-border bg-surface-2/60 px-3 py-2 text-sm text-muted">
            Only the workspace owner can manage billing.
          </p>
        {/if}
        {#if checkoutError}
          <p class="mb-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
            {checkoutError}
          </p>
        {/if}

        <Card class="flex flex-col gap-6 p-6">
          {#if isFree}
            <!-- What a seat unlocks beyond the Free tier, before the pricing. -->
            <div>
              <div class="text-sm font-medium text-text">What upgrading unlocks</div>
              <ul class="mt-2 grid gap-x-6 gap-y-1.5 text-sm text-muted sm:grid-cols-2">
                <li>✓ Full revision history — nothing gets replaced</li>
                <li>✓ Share links that never expire</li>
                <li>✓ {PER_SEAT.shareLinks} active share links per seat</li>
                <li>✓ Teammates — 1 member per seat</li>
                <li>✓ {PER_SEAT.storageGib} GiB storage per seat</li>
                <li>✓ Storage add-ons when you need more</li>
              </ul>
            </div>
            <hr class="border-border" />
          {/if}

          <div>
            <div class="text-base font-semibold">Seat plan</div>
            <p class="mt-1 max-w-prose text-sm text-muted">
              €{SEAT_FIRST_EUR}/mo for the first seat, €{SEAT_ADDITIONAL_EUR}/mo for each additional
              seat. Scale up or down anytime — your Free workspace and everything in it carries
              over.
            </p>
          </div>

          <!-- Monthly / Yearly toggle -->
          <div class="inline-flex w-fit items-center rounded-md border border-border p-0.5 text-sm">
            {#each INTERVALS as iv (iv)}
              <button
                type="button"
                class="rounded px-3 py-1 transition {interval === iv
                  ? 'bg-surface-2 text-text'
                  : 'text-muted hover:text-text'}"
                aria-pressed={interval === iv}
                onclick={() => (interval = iv)}
              >
                {iv === 'monthly' ? 'Monthly' : 'Yearly'}
              </button>
            {/each}
            <span class="self-center px-2 text-xs text-accent">2 months free</span>
          </div>

          <!-- Seat stepper -->
          <div class="flex flex-wrap items-center gap-4">
            <span class="text-sm text-muted">Seats</span>
            {@render stepperControl(
              String(seats),
              () => stepSeats(-1),
              () => stepSeats(1),
              !isOwner || seats <= SEATS_MIN,
              !isOwner || seats >= SEATS_MAX,
              'Fewer seats',
              'More seats'
            )}
          </div>

          {@render entitlements(seats)}

          <hr class="border-border" />

          <!-- Optional storage add-on, bundled into the same checkout -->
          <div class="flex flex-col gap-3">
            <div>
              <div class="text-sm font-medium text-text">Add storage (optional)</div>
              <p class="mt-1 max-w-prose text-sm text-muted">
                Extra room for math blobs in {STORAGE_UNIT_GIB} GiB units for €1/mo each — added to
                the same subscription. Leave at 0 to skip.
              </p>
            </div>
            <div class="flex flex-wrap items-center gap-4">
              {@render stepperControl(
                checkoutStorage > 0 ? `+${checkoutStorage * STORAGE_UNIT_GIB} GiB` : 'None',
                () => stepCheckoutStorage(-1),
                () => stepCheckoutStorage(1),
                !isOwner || checkoutStorage <= 0,
                !isOwner || checkoutStorage >= STORAGE_UNITS_MAX,
                'Less storage',
                'More storage',
                '7rem'
              )}
              <span class="text-sm text-muted">
                <span class="font-semibold text-text">€{storageMonthlyEur(checkoutStorage)} / mo</span>
              </span>
            </div>
          </div>

          <!-- Live combined price -->
          {@render priceBox(seats, checkoutStorage, interval)}

          <Button
            class="w-fit"
            loading={checkoutBusy}
            disabled={!isOwner || checkoutBusy}
            onclick={subscribe}
          >
            Subscribe
          </Button>
        </Card>

        <p class="mt-4 text-xs text-faint">
          Prices exclude tax — VAT, when applicable, is added at checkout based on your country.
          Payments are processed securely by Stripe as merchant of record.
        </p>
      </section>
    {/if}
  {/if}
</main>
