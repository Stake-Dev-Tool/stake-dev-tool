<script lang="ts">
  import { page } from '$app/state';
  import { replaceState } from '$app/navigation';
  import { api, type BillingStatus, type BillingInterval, type PlanId, type Role } from '$lib/api';
  import {
    billingStatus,
    setBillingStatus,
    planLabel,
    statusLabel,
    intervalLabel,
    meter,
    meterFill,
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

  let checkoutError = $state('');
  let checkoutBusy = $state<PlanId | null>(null);

  // Storage add-on stepper (one unit = +10 GiB for €1/mo).
  let storageUnits = $state(1);
  let storageBusy = $state(false);
  let storageError = $state('');

  // Per-card Monthly/Yearly toggle.
  let intervals = $state<Record<PlanId, BillingInterval>>({ solo: 'monthly', team: 'monthly' });
  const INTERVALS: BillingInterval[] = ['monthly', 'yearly'];

  let isOwner = $derived(role === 'owner');

  // The two purchasable plans (limits + indicative pricing from V2.md).
  const PLANS: {
    id: PlanId;
    name: string;
    blurb: string;
    features: string[];
    price: Record<BillingInterval, string>;
  }[] = [
    {
      id: 'solo',
      name: 'Solo',
      blurb: 'A single developer, unlimited games.',
      features: ['1 member', '10 GiB math storage', '5 active share links'],
      price: { monthly: '€5 / mo', yearly: '€48 / yr' }
    },
    {
      id: 'team',
      name: 'Team',
      blurb: 'Up to ten seats and higher share quotas.',
      features: ['10 members', '50 GiB math storage', '25 active share links'],
      price: { monthly: '€15 / mo', yearly: '€144 / yr' }
    }
  ];

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
    if (status.plan === 'trial') return `Trial ends ${d}`;
    if (status.plan === 'expired') return `Trial ended ${d}`;
    if (status.status === 'canceled') return `Access until ${d}`;
    return `Renews ${d}`;
  });

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

      // Stripe success redirect (?upgraded=1): celebrate, refetch fresh (the
      // cache may predate the subscription), and strip the param. Read here —
      // after the await — so this effect never subscribes to page.url.
      if (page.url.searchParams.get('upgraded') === '1') {
        toast.success('Subscription active — welcome aboard.');
        try {
          const fresh = await api.billing.status(slug);
          setBillingStatus(slug, fresh);
          status = fresh;
        } catch {
          // Keep the cached status; the toast still stands.
        }
        const url = new URL(page.url);
        url.searchParams.delete('upgraded');
        replaceState(url.pathname + url.search, {});
      }
    } catch (e) {
      loadError = errorText(e);
    } finally {
      loading = false;
    }
  }

  async function upgrade(plan: PlanId) {
    if (!isOwner || checkoutBusy) return;
    checkoutBusy = plan;
    checkoutError = '';
    try {
      const url = await api.billing.checkout(slug, plan, intervals[plan]);
      if (url) {
        window.location.href = url; // full navigation to the hosted checkout
        return; // leave the button busy; the page is unloading
      }
      checkoutError = 'Checkout is unavailable right now. Please try again.';
    } catch (e) {
      checkoutError = errorText(e);
    }
    checkoutBusy = null;
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

  <h1 class="mb-8 text-2xl font-semibold tracking-tight">Billing</h1>

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
          {#if status.status}
            <Badge tone={status.status === 'past_due' ? 'warn' : status.plan === 'expired' ? 'danger' : 'accent'}>
              {statusLabel(status.status)}
            </Badge>
          {:else if status.plan === 'expired'}
            <Badge tone="danger">Expired</Badge>
          {:else if status.plan === 'trial'}
            <Badge tone="info">Trialing</Badge>
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

    <!-- Usage vs limits -->
    <section class="mb-8">
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">Usage</h2>
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
    </section>

    <!-- Upgrade -->
    <section>
      <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-faint">
        {status.plan === 'solo' || status.plan === 'team' ? 'Change plan' : 'Upgrade'}
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

      <div class="grid gap-4 sm:grid-cols-2">
        {#each PLANS as p (p.id)}
          {@const active = status.plan === p.id}
          <Card class="flex flex-col gap-4 p-6">
            <div class="flex items-center justify-between gap-2">
              <span class="text-base font-semibold">{p.name}</span>
              {#if active}<Badge tone="accent">Current</Badge>{/if}
            </div>
            <p class="text-sm text-muted">{p.blurb}</p>

            <ul class="flex flex-col gap-1.5 text-sm">
              {#each p.features as f (f)}
                <li class="flex items-center gap-2 text-muted">
                  <span class="text-accent" aria-hidden="true">✓</span>
                  {f}
                </li>
              {/each}
            </ul>

            <!-- Monthly / Yearly toggle -->
            <div class="inline-flex rounded-md border border-border p-0.5 text-sm">
              {#each INTERVALS as iv (iv)}
                <button
                  type="button"
                  class="rounded px-3 py-1 transition {intervals[p.id] === iv
                    ? 'bg-surface-2 text-text'
                    : 'text-muted hover:text-text'}"
                  aria-pressed={intervals[p.id] === iv}
                  onclick={() => (intervals[p.id] = iv)}
                >
                  {iv === 'monthly' ? 'Monthly' : 'Yearly'}
                </button>
              {/each}
            </div>

            <div>
              <div class="text-lg font-semibold">{p.price[intervals[p.id]]}</div>
              {#if intervals[p.id] === 'yearly'}
                <div class="text-xs text-accent">2 months free</div>
              {/if}
            </div>

            <Button
              class="w-full"
              loading={checkoutBusy === p.id}
              disabled={!isOwner || checkoutBusy !== null}
              onclick={() => upgrade(p.id)}
            >
              {active ? 'Switch billing' : `Upgrade to ${p.name}`}
            </Button>
          </Card>
        {/each}
      </div>

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
