<script lang="ts">
  type Props = { value: string; label?: string; class?: string };
  let { value, label, class: klass = '' }: Props = $props();

  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      copied = true;
      clearTimeout(timer);
      timer = setTimeout(() => (copied = false), 1600);
    } catch {
      copied = false;
    }
  }
</script>

<div class="flex flex-col gap-1.5 {klass}">
  {#if label}
    <span class="text-sm font-medium text-muted">{label}</span>
  {/if}
  <div class="flex items-stretch gap-2">
    <code
      class="min-w-0 flex-1 truncate rounded-md border border-border bg-surface-2 px-3 py-2 font-mono-tab text-sm text-text"
      title={value}>{value}</code
    >
    <button
      type="button"
      onclick={copy}
      class="shrink-0 rounded-md border border-border bg-surface-2 px-3 text-sm text-muted transition hover:border-border-strong hover:text-text"
    >
      {copied ? 'Copied' : 'Copy'}
    </button>
  </div>
</div>
