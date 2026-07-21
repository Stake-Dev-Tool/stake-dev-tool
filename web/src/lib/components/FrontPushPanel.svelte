<script lang="ts">
  /**
   * FrontPushPanel — upload the game-front build (the web bundle players load)
   * as a front bundle. Lives next to MathPushPanel on the game page so both
   * push actions share one home; shares and the hosted test view use the
   * latest bundle automatically.
   */
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import MathFolderPicker from '$lib/components/MathFolderPicker.svelte';
  import UpgradeNotice from '$lib/components/UpgradeNotice.svelte';
  import { humanSize } from '$lib/format';
  import { isUpgradeError } from '$lib/api';
  import {
    runFrontPush,
    pushErrorMessage,
    type FileProgress,
    type IntakeFile,
    type PushPhase
  } from '$lib/push';

  type Props = {
    slug: string;
    game: string;
    /** Fired with the new bundle id after a successful upload. */
    onuploaded?: (bundleId: string) => void;
    oncancel?: () => void;
  };

  let { slug, game, onuploaded, oncancel }: Props = $props();

  let files = $state<IntakeFile[]>([]);
  let pickerKey = $state(0); // bump to remount the picker (clears its selection)
  let pushing = $state(false);
  let error = $state('');
  let errorUpgrade = $state(false);
  let phase = $state<PushPhase | null>(null);
  let progress = $state<FileProgress[]>([]);
  let bundleId = $state<string | null>(null);

  let canPush = $derived(files.length > 0 && !pushing);

  // Compact recap (no per-file table — a front build is many small files).
  let total = $derived(progress.length);
  let hashed = $derived(progress.filter((p) => p.status !== 'pending' && p.status !== 'hashing').length);
  let processed = $derived(
    progress.filter((p) => p.status === 'uploaded' || p.status === 'deduplicated').length
  );
  let sent = $derived(progress.filter((p) => p.status === 'uploaded').reduce((a, p) => a + p.size, 0));
  let dedup = $derived(progress.filter((p) => p.status === 'deduplicated').length);
  let phaseLabel = $derived.by(() => {
    switch (phase) {
      case 'hashing':
        return `Hashing files… ${hashed} / ${total}`;
      case 'checking':
        return 'Checking which files the server already has…';
      case 'uploading':
        return `Uploading… ${processed} / ${total} files · ${humanSize(sent)} sent · ${dedup} deduplicated`;
      case 'committing':
        return 'Committing front bundle…';
      case 'done':
        return 'Done.';
      default:
        return '';
    }
  });

  async function doPush() {
    if (!canPush) return;
    pushing = true;
    error = '';
    errorUpgrade = false;
    bundleId = null;
    phase = 'hashing';
    progress = files.map((f) => ({
      path: f.path,
      size: f.file.size,
      status: 'pending' as const,
      hashedBytes: 0
    }));
    try {
      const res = await runFrontPush(
        { slug, game, files },
        {
          onPhase: (p) => (phase = p),
          onFileUpdate: (i, patch) => {
            const cur = progress[i];
            if (cur) progress[i] = { ...cur, ...patch };
          }
        }
      );
      bundleId = res.bundleId;
      files = [];
      pickerKey += 1;
      onuploaded?.(res.bundleId);
    } catch (e) {
      error = pushErrorMessage(e);
      errorUpgrade = isUpgradeError(e);
      phase = null;
    } finally {
      pushing = false;
    }
  }
</script>

<Card class="p-6">
  <div class="mb-1 flex flex-wrap items-center justify-between gap-3">
    <h3 class="text-base font-semibold">Push the game front</h3>
    <span class="text-xs text-faint">index.html at root · up to 2000 files · relative base</span>
  </div>
  <p class="mb-4 text-sm text-muted">
    The web build players load in the browser. Share links and the hosted test view serve the
    latest bundle automatically.
  </p>

  {#key pickerKey}
    <MathFolderPicker
      disabled={pushing}
      requiredRootFile="index.html"
      maxFiles={2000}
      label="Drop your game-front build here"
      onpicked={(f) => {
        files = f;
        error = '';
      }}
      oncleared={() => (files = [])}
    />
  {/key}

  {#if error}
    {#if errorUpgrade}
      <UpgradeNotice {slug} message={error} class="mt-4" />
    {:else}
      <p class="mt-4 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
        {error}
      </p>
    {/if}
  {/if}

  {#if phase}
    <div class="mt-4 flex flex-col gap-3 rounded-md border border-border bg-surface-2/40 p-4">
      <div class="flex items-center gap-2.5 text-sm">
        {#if phase !== 'done'}<span class="spinner text-accent"></span>{/if}
        <span class="text-text">{phaseLabel}</span>
      </div>
      <div class="h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
        <div
          class="h-full rounded-full bg-accent transition-all"
          style="width: {total > 0 ? Math.round((processed / total) * 100) : 0}%"
        ></div>
      </div>
    </div>
  {/if}

  {#if bundleId}
    <div
      class="mt-4 flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border border-accent/30 bg-accent/10 px-3 py-2.5 text-sm text-accent"
    >
      <span class="font-medium">Front bundle uploaded.</span>
      <span class="text-muted">New shares and the test view use it automatically.</span>
      <span class="font-mono-tab text-xs text-faint">id {bundleId.slice(0, 8)}</span>
    </div>
  {/if}

  <div class="mt-4 flex gap-3">
    <Button onclick={doPush} loading={pushing} disabled={!canPush}>Upload front build</Button>
    {#if oncancel}
      <Button variant="secondary" onclick={() => oncancel?.()}>Close</Button>
    {/if}
  </div>
</Card>
