<script lang="ts">
  // Desktop chrome layout: covers the launcher (`/`) and `/teams` but NOT
  // `/test`. The math-sync progress overlay listens on a Tauri event, so it
  // lives here (out of the shared root layout) to keep the `/test` chunk free
  // of any `@tauri-apps` import.
  //
  // The `brand` class scopes the site palette to the chrome only — the test
  // view (embedded identically on the cloud) keeps its original look. On
  // <html> so shadcn portals (dialogs/sheets, mounted on <body>) inherit it.
  import { onMount } from 'svelte';
  import MathSyncOverlay from '$lib/components/MathSyncOverlay.svelte';
  let { children } = $props();

  onMount(() => {
    document.documentElement.classList.add('brand');
    return () => document.documentElement.classList.remove('brand');
  });
</script>

{@render children()}
<MathSyncOverlay />
