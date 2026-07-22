<script lang="ts">
  import { page } from '$app/state';
  import Button from '$lib/components/Button.svelte';
</script>

<svelte:head><title>Error · Stake Dev Tool Cloud</title></svelte:head>

<main class="flex min-h-screen flex-col items-center justify-center gap-4 px-6 text-center">
  <span class="font-mono-tab text-5xl font-bold text-faint">{page.status}</span>
  <h1 class="text-lg font-semibold">{page.error?.message ?? 'Something went wrong'}</h1>
  <Button href="/" variant="outline">Back to Stake Dev Tool Cloud</Button>
</main>
