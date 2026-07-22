<script lang="ts">
  import { page } from '$app/state';
  import { goto, replaceState } from '$app/navigation';
  import { api, type BillingStatus, type BillingInterval, type Role } from '$lib/api';
  import {
    billingStatus,
    setBillingStatus,
    planLabel,
    statusLabel,
    intervalLabel,
    meter,
    meterFill,
    clampSeats,
    seatMonthlyEur,
    seatYearlyEur,
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

  let checkoutError = $state('');
  let checkoutBusy = $state(false);

  // Seat subscription: a stepper (1..100) + a monthly/yearly toggle.
  let seats = $state(1);
  let interval = $state<BillingInterval>('monthly');
  let seatsSeeded = false;
  const INTERVALS: BillingInterval[] = ['monthly', 'yearly'];

  // Storage add-on stepper (one unit = +10 GiB for €1/mo).
  let storageUnits = $state(1);
  let storageBusy = $state(false);
  let storageError = $state('');

  let isOwner = $derived(role === 'owner');
  let isFree = $derived(status?.plan === 'free');
  let isPaid = $derived(status?.plan === 'paid');

  // Live seat pricing.
  let monthlyPrice = $derived(seatMonthlyEur(seats));
  let yearlyPrice = $derived(seatYearlyEur(seats));

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
    if (status.status === 'canceled') return `Access until ${d}`;
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

  async function subscribe() {
    if (!isOwner || checkoutBusy) return;
    checkoutBusy = true;
    checkoutError = '';
    try {
      const url = await api.billing.checkout(slug, interval, clampSeats(seats));
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

  function stepStorage(delta: number) {
    storageUnits = clampStorageUnits(storageUnits + delta);
  }

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
</script>

<svelte:head><title>Billing · {wsName || workspaceName(slug)} · Stake Dev Tool Cloud</title></svelte:head>

<main class="mx-auto w-full max-w-4xl px-6 py-10">
  <Breadcrumbs
    items={[{ label: wsName || workspaceName(slug), href: `/w/${slug}` }, { label: 'Billing' }]}
  />

  {#if activating}
    <div class="mb-8">
      <h1 class="text-2xl font-semibold tracking-tight">Activate {wsName || workspaceName(slug)}</h1>
      <p class="mt-2 max-w-prose text-sm leading-relaxed text-muted">
        Subscribe to start pushing math, inviting your team and sharing games.
      </p>
      <Button variant="outline" size="sm" class="mt-4" onclick={payLater}>Pay later</Button>
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
            <Badge tone="danger">No plan</Badge>
          {/if}
          {#if status.interval}
            <span class="text-sm text-muted">· {intervalLabel(status.interval)}</span>
          {/if}
        </div>
        {#if periodLine}
          <p class="mt-1.5 text-sm text-muted">{periodLine}</p>
        {/if}
      </Card>
    </section>

    <!-- Usage -->
    <section class="mb-8">
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Usage</h2>
      {#if isFree}
        <!-- Free: no caps to show (all limits are 0). Report plain facts, then a
             single line explaining that everything is locked until subscribed. -->
        <Card class="flex flex-col gap-4 p-6">
          <ul class="flex flex-col gap-2 text-sm">
            <li class="flex items-baseline justify-between gap-3">
              <span class="text-muted">Members</span>
              <span class="font-mono-tab text-text">{plural(status.usage.members, 'member')}</span>
            </li>
            <li class="flex items-baseline justify-between gap-3">
              <span class="text-muted">Storage</span>
              <span class="font-mono-tab text-text">{humanSize(status.usage.storage_bytes)} used</span>
            </li>
            <li class="flex items-baseline justify-between gap-3">
              <span class="text-muted">Share links</span>
              <span class="font-mono-tab text-text">
                {plural(status.usage.active_share_links, 'share link')}
                <span class="text-faint">(inactive)</span>
              </span>
            </li>
          </ul>
          <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
            No active plan — pushes, invites and new share links are locked, and existing share
            links stop serving new play sessions until you subscribe. Your content stays readable.
          </p>
        </Card>
      {:else}
        <!-- Active plan: meters against the seat-derived caps. -->
        <Card class="flex flex-col gap-5 p-6">
          {#each meters as m (m.label)}
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
          {/each}
        </Card>
      {/if}
    </section>

    <!-- Subscribe / change seats -->
    <section>
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">
        {isPaid ? 'Change plan' : 'Subscribe'}
      </h2>

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

      <Card class="flex flex-col gap-5 p-6">
        <div>
          <div class="text-base font-semibold">Seat plan</div>
          <p class="mt-1 max-w-prose text-sm text-muted">
            €{SEAT_FIRST_EUR}/mo for the first seat, €{SEAT_ADDITIONAL_EUR}/mo for each additional
            seat. Every seat adds a member slot plus {STORAGE_UNIT_GIB} GiB storage, 5 share links
            and 5 live play sessions.
          </p>
        </div>

        <!-- Monthly / Yearly toggle -->
        <div class="inline-flex w-fit rounded-md border border-border p-0.5 text-sm">
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
          <div class="inline-flex items-center rounded-md border border-border">
            <button
              type="button"
              class="px-3 py-1.5 text-lg leading-none text-muted transition hover:text-text disabled:opacity-40"
              aria-label="Fewer seats"
              disabled={!isOwner || seats <= SEATS_MIN}
              onclick={() => stepSeats(-1)}
            >
              −
            </button>
            <span class="min-w-[4rem] px-3 text-center font-mono-tab text-sm font-medium text-text">
              {seats}
            </span>
            <button
              type="button"
              class="px-3 py-1.5 text-lg leading-none text-muted transition hover:text-text disabled:opacity-40"
              aria-label="More seats"
              disabled={!isOwner || seats >= SEATS_MAX}
              onclick={() => stepSeats(1)}
            >
              +
            </button>
          </div>
        </div>

        <!-- Live price -->
        <div>
          <div class="text-sm text-muted">
            €{SEAT_FIRST_EUR}
            {#if seats > 1}
              + €{SEAT_ADDITIONAL_EUR} × {seats - 1}
            {/if}
            =
            <span class="font-semibold text-text">€{monthlyPrice} / mo</span>
          </div>
          {#if interval === 'yearly'}
            <div class="mt-0.5 text-sm">
              <span class="font-semibold text-text">€{yearlyPrice} / yr</span>
              <span class="text-accent">· 2 months free</span>
            </div>
          {/if}
        </div>

        <Button
          class="w-fit"
          loading={checkoutBusy}
          disabled={!isOwner || checkoutBusy}
          onclick={subscribe}
        >
          {isPaid ? 'Update subscription' : 'Subscribe'}
        </Button>
      </Card>

      <p class="mt-4 text-xs text-faint">
        Prices exclude tax — VAT, when applicable, is added at checkout based on your country. Payments
        are processed securely by Stripe as merchant of record.
      </p>
    </section>

    <!-- Storage add-on -->
    <section class="mt-8">
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Storage add-on</h2>
      <Card class="flex flex-col gap-4 p-6">
        <p class="max-w-prose text-sm text-muted">
          Need more room for math blobs? Add storage in {STORAGE_UNIT_GIB} GiB units for €1/mo each.
          It stacks on top of your plan's storage cap.
        </p>

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
          <div class="inline-flex items-center rounded-md border border-border">
            <button
              type="button"
              class="px-3 py-1.5 text-lg leading-none text-muted transition hover:text-text disabled:opacity-40"
              aria-label="Fewer units"
              disabled={!isOwner || storageUnits <= STORAGE_UNITS_MIN}
              onclick={() => stepStorage(-1)}
            >
              −
            </button>
            <span class="min-w-[7rem] px-3 text-center text-sm font-mono-tab font-medium text-text">
              +{storageUnits * STORAGE_UNIT_GIB} GiB
            </span>
            <button
              type="button"
              class="px-3 py-1.5 text-lg leading-none text-muted transition hover:text-text disabled:opacity-40"
              aria-label="More units"
              disabled={!isOwner || storageUnits >= STORAGE_UNITS_MAX}
              onclick={() => stepStorage(1)}
            >
              +
            </button>
          </div>

          <span class="text-sm text-muted">
            <span class="font-semibold text-text">€{storageMonthlyEur(storageUnits)} / mo</span>
          </span>

          <Button loading={storageBusy} disabled={!isOwner || storageBusy} onclick={buyStorage}>
            {status.extra_storage_gib > 0 ? 'Add more storage' : 'Add storage'}
          </Button>
        </div>

        {#if !isOwner}
          <p class="text-xs text-faint">Only the workspace owner can buy storage.</p>
        {/if}
      </Card>
    </section>
  {/if}
</main>
