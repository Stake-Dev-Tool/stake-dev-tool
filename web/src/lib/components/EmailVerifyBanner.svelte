<script module lang="ts">
  // Dismissed at the module level so it persists for the whole browser session
  // (across route changes / component remounts), not just one mount.
  let dismissed = $state(false);
</script>

<script lang="ts">
  import { api, ApiError } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import { toast } from '$lib/toasts.svelte';

  let sending = $state(false);

  let show = $derived(!!session.user && !session.user.email_verified && !dismissed);

  async function resend() {
    if (sending) return;
    sending = true;
    try {
      await api.auth.resendVerification();
      toast.success('Verification email sent. Check your inbox.');
    } catch (e) {
      if (e instanceof ApiError && e.status === 429) {
        toast.error('Too many requests. Please wait a moment and try again.');
      } else {
        toast.error('Could not send the verification email. Please try again.');
      }
    } finally {
      sending = false;
    }
  }
</script>

{#if show}
  <div class="border-b border-warn/30 bg-warn/10">
    <div
      class="mx-auto flex w-full max-w-5xl flex-wrap items-center gap-x-3 gap-y-1.5 px-6 py-2 text-sm"
    >
      <span class="text-warn">⚠</span>
      <span class="text-text">
        Verify your email address to finish setting up your account and create workspaces.
      </span>
      <button
        type="button"
        onclick={resend}
        disabled={sending}
        class="font-medium text-accent underline-offset-4 transition hover:underline disabled:opacity-50"
      >
        {sending ? 'Sending…' : 'Resend link'}
      </button>
      <button
        type="button"
        onclick={() => (dismissed = true)}
        aria-label="Dismiss"
        class="ml-auto text-muted transition hover:text-text"
      >
        ✕
      </button>
    </div>
  </div>
{/if}
