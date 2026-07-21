<script lang="ts">
  /**
   * Tabs — a horizontal, underline-style tab bar. Purely presentational: the
   * parent owns the active id and swaps panels; this just renders the buttons
   * (with an optional count badge) and reports selection. Deep-linking to a tab
   * (via #hash) is the parent's job.
   */
  export type TabItem = { id: string; label: string; badge?: string | number };

  type Props = { tabs: TabItem[]; active: string; onselect: (id: string) => void; class?: string };
  let { tabs, active, onselect, class: klass = '' }: Props = $props();
</script>

<div role="tablist" class="flex gap-1 overflow-x-auto border-b border-border {klass}">
  {#each tabs as t (t.id)}
    <button
      type="button"
      role="tab"
      aria-selected={active === t.id}
      class="relative -mb-px flex items-center gap-2 whitespace-nowrap border-b-2 px-4 py-2.5 text-sm font-medium transition {active ===
      t.id
        ? 'border-accent text-text'
        : 'border-transparent text-muted hover:text-text'}"
      onclick={() => onselect(t.id)}
    >
      {t.label}
      {#if t.badge != null}
        <span class="rounded-full bg-surface-2 px-1.5 py-0.5 text-xs text-faint">{t.badge}</span>
      {/if}
    </button>
  {/each}
</div>
