<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    class: klass = '',
    children,
    ...rest
  }: { class?: string; children?: Snippet; [key: string]: unknown } = $props();
</script>

<div class="rounded-lg border border-border bg-surface {klass}" {...rest}>
  {@render children?.()}
</div>
