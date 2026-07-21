<script lang="ts">
  // Desktop chrome layout: covers the launcher (`/`) and `/teams` but NOT
  // `/test`. The math-sync progress overlay listens on a Tauri event, so it
  // lives here (out of the shared root layout) to keep the `/test` chunk free
  // of any `@tauri-apps` import.
  import MathSyncOverlay from '$lib/components/MathSyncOverlay.svelte';
  let { children } = $props();
</script>

{@render children()}
<MathSyncOverlay />
