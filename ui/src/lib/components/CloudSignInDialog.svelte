<script lang="ts">
  import { onDestroy } from 'svelte';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { Button } from '$lib/components/ui/button';
  import * as Dialog from '$lib/components/ui/dialog';
  import { toast } from 'svelte-sonner';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';

  import { cloudApi, type CloudDeviceCode, type CloudUser } from '$lib/api';

  let {
    open = $bindable(false),
    onSignedIn
  }: {
    open?: boolean;
    onSignedIn?: (user: CloudUser) => void;
  } = $props();

  let deviceCode = $state<CloudDeviceCode | null>(null);
  let polling = $state(false);
  let pollTimeout: number | null = null;
  let pollCancelled = false;
  let currentPollInterval = 5;

  // Re-run the device flow each time the dialog is opened from the closed state.
  let prevOpen = false;
  $effect(() => {
    if (open && !prevOpen) {
      startDeviceFlow();
    } else if (!open && prevOpen) {
      cancelPollTimer();
      polling = false;
      deviceCode = null;
    }
    prevOpen = open;
  });

  onDestroy(() => cancelPollTimer());

  function cancelPollTimer() {
    pollCancelled = true;
    if (pollTimeout !== null) {
      clearTimeout(pollTimeout);
      pollTimeout = null;
    }
  }

  async function startDeviceFlow() {
    try {
      pollCancelled = false;
      deviceCode = await cloudApi.requestDeviceCode();
      polling = true;
      currentPollInterval = Math.max(5, deviceCode.interval);
      try {
        await openUrl(deviceCode.verification_uri);
      } catch {
        /* user opens manually */
      }
      scheduleNextPoll(currentPollInterval);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
      open = false;
    }
  }

  function scheduleNextPoll(delaySeconds: number) {
    if (pollCancelled) return;
    if (pollTimeout !== null) clearTimeout(pollTimeout);
    pollTimeout = window.setTimeout(pollOnce, delaySeconds * 1000);
  }

  async function pollOnce() {
    pollTimeout = null;
    if (pollCancelled || !deviceCode) return;
    try {
      const result = await cloudApi.pollForToken(deviceCode.device_code, currentPollInterval);
      currentPollInterval = result.next_interval_secs;
      if (result.auth) {
        cancelPollTimer();
        polling = false;
        const user = result.auth.user;
        toast.success(`Signed in as ${user.display_name}`);
        open = false;
        onSignedIn?.(user);
        return;
      }
      scheduleNextPoll(currentPollInterval);
    } catch (e) {
      cancelPollTimer();
      polling = false;
      open = false;
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }

  async function copyDeviceCode() {
    if (!deviceCode) return;
    try {
      await navigator.clipboard.writeText(deviceCode.user_code);
      toast.success('Code copied');
    } catch {
      toast.error('Could not copy to clipboard');
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>Sign in to the cloud</Dialog.Title>
      <Dialog.Description>
        {#if deviceCode}
          A browser window opened to {deviceCode.verification_uri}. Enter this code to approve this
          device:
        {:else}
          Requesting a device code…
        {/if}
      </Dialog.Description>
    </Dialog.Header>
    {#if deviceCode}
      <div class="my-4 flex items-center justify-center gap-2">
        <code class="font-mono-tab rounded-md border bg-muted px-4 py-3 text-2xl tracking-[0.35em]">
          {deviceCode.user_code}
        </code>
        <Button variant="ghost" size="icon" onclick={copyDeviceCode}>
          <CopyIcon />
        </Button>
      </div>
      <div class="flex items-center justify-center gap-2 text-xs text-muted-foreground">
        {#if polling}
          <span class="h-2 w-2 animate-pulse rounded-full bg-blue-500"></span>
          Waiting for you to approve…
        {/if}
      </div>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>Cancel</Button>
      {#if deviceCode}
        <Button variant="secondary" onclick={() => openUrl(deviceCode!.verification_uri)}>
          <ExternalLinkIcon />
          Reopen approval page
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
