<script lang="ts">
  /**
   * PlanBanner — the per-workspace billing nudge. Self-fetches (once per slug per
   * session, via the shared `billingStatus` cache) and renders:
   *   • nothing            when billing is disabled (self-host) or unknown;
   *   • a prominent banner when the workspace has no active plan (Free — writes blocked);
   *   • a warning banner   when the subscription is past_due (grace period);
   *   • a tiny plan chip   on a healthy Solo/Team plan (no banner).
   *
   * Mounted on the workspace page and both game pages; pass `slug`, it does the
   * rest. Never throws up the tree — a failed fetch simply renders nothing.
   */
  import type { BillingStatus } from '$lib/api';
  import { billingStatus } from '$lib/billing';
  import { formatDate } from '$lib/format';

  let { slug }: { slug: string } = $props();

  let st = $state<BillingStatus | null>(null);

  $effect(() => {
    const s = slug;
    st = null;
    if (!s) return;
    let cancelled = false;
    billingStatus(s)
      .then((r) => {
        if (!cancelled) st = r;
      })
      .catch(() => {
        if (!cancelled) st = null;
      });
    return () => {
      cancelled = true;
    };
  });

  let billingHref = $derived(`/w/${slug}/billing`);
</script>

{#if st && st.enabled}
  {#if st.plan === 'free'}
    <div
      class="mb-6 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-lg border border-danger/40 bg-danger/10 px-4 py-3 text-sm"
    >
      <span class="text-danger">
        <span class="font-semibold">No active plan</span>
        — pushes, invites and new share links require a subscription.
      </span>
      <a
        href={billingHref}
        class="font-semibold text-danger underline-offset-4 hover:underline"
      >
        Choose a plan →
      </a>
    </div>
  {:else if st.status === 'past_due'}
    <div
      class="mb-6 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-lg border border-warn/40 bg-warn/10 px-4 py-3 text-sm"
    >
      <span class="text-warn">
        <span class="font-semibold">Payment issue</span>
        — update billing to keep access{#if st.current_period_end}
          · grace until {formatDate(st.current_period_end)}{/if}.
      </span>
      <a href={billingHref} class="font-semibold text-warn underline-offset-4 hover:underline">
        Fix billing →
      </a>
    </div>
  {:else if st.plan === 'solo' || st.plan === 'team'}
    <a
      href={billingHref}
      class="mb-6 inline-flex items-center gap-1 rounded-full border border-border bg-surface-2 px-2.5 py-0.5 text-xs font-medium text-muted transition hover:text-text"
      title="Manage billing"
    >
      <span class="h-1.5 w-1.5 rounded-full bg-accent"></span>
      {st.plan === 'team' ? 'Team' : 'Solo'} plan
    </a>
  {/if}
{/if}
