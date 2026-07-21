<script lang="ts">
  import { page } from '$app/state';
  import TestView from '$lib/testview/TestView.svelte';
  import { resolveContext } from '$lib/testview/context';

  // Thin wrapper around the shared TestView. `resolveContext` picks the context
  // off this window's URL: on the cloud server the SAME embedded page is served
  // under a tenant prefix (`/api/ws/:slug/g/:game/r/:number/test/`) and gets a
  // prefix-aware cloud context; the desktop launcher (`/test/`) gets the
  // byte-identical desktop context. Params (gameUrl, gameSlug) come from the
  // query string in both cases; `page.url` supplies both the path (prefix
  // discriminator) and the host (RGS base).
  const ctx = resolveContext(page.url.searchParams, page.url);
</script>

<TestView {ctx} />
