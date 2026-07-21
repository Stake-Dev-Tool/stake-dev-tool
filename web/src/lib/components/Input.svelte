<script lang="ts">
  type Props = {
    value?: string;
    label?: string;
    id?: string;
    error?: string;
    hint?: string;
    mono?: boolean;
    class?: string;
    [key: string]: unknown;
  };

  let {
    value = $bindable(''),
    label,
    id,
    error,
    hint,
    mono = false,
    class: klass = '',
    ...rest
  }: Props = $props();
</script>

<div class="flex flex-col gap-1.5 {klass}">
  {#if label}
    <label for={id} class="text-sm font-medium text-muted">{label}</label>
  {/if}
  <input
    {id}
    bind:value
    class="h-9 w-full rounded-md border bg-surface-2 px-3 text-sm text-text outline-none transition placeholder:text-faint focus:ring-2 focus:ring-accent/25 {mono
      ? 'font-mono-tab'
      : ''} {error ? 'border-danger/70 focus:border-danger/70' : 'border-border focus:border-accent/60'}"
    {...rest}
  />
  {#if error}
    <p class="text-xs text-danger">{error}</p>
  {:else if hint}
    <p class="text-xs text-faint">{hint}</p>
  {/if}
</div>
