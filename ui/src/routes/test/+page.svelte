<script lang="ts">
  import { page } from '$app/state';
  import TestView from '$lib/testview/TestView.svelte';
  import { desktopContext } from '$lib/testview/context';

  // Desktop test view: build the injection context from the launcher's query
  // params (gameUrl, gameSlug) and this window's location, then render the
  // shared TestView. Kept intentionally thin — the same TestView is meant to be
  // driven by cloud/share contexts in the web workbench (M6).
  const ctx = desktopContext(page.url.searchParams, location);
</script>

<TestView {ctx} />
