<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { Badge } from '$lib/components/ui/badge';
  import * as Card from '$lib/components/ui/card';
  import UploadIcon from '@lucide/svelte/icons/upload';
  import DownloadCloudIcon from '@lucide/svelte/icons/download-cloud';
  import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';
  import HashIcon from '@lucide/svelte/icons/hash';

  type Progress = {
    gameSlug: string;
    phase: 'hashing' | 'uploading' | 'downloading' | 'committing' | 'done';
    currentFile: string;
    fileIndex: number;
    fileCount: number;
    bytesDone: number;
    bytesTotal: number;
  };

  let progress = $state<Progress | null>(null);
  let hideTimer: number | null = null;
  let startedAt = $state<number | null>(null);
  let unlisten: UnlistenFn | null = null;

  onMount(() => {
    (async () => {
      unlisten = await listen<Progress>('math-sync-progress', (ev) => {
        if (hideTimer !== null) {
          clearTimeout(hideTimer);
          hideTimer = null;
        }
        if (progress === null || progress.gameSlug !== ev.payload.gameSlug) {
          startedAt = Date.now();
        }
        progress = ev.payload;
        if (ev.payload.phase === 'done') {
          hideTimer = window.setTimeout(() => {
            progress = null;
            startedAt = null;
          }, 2500);
        }
      });
    })();
  });

  onDestroy(() => {
    if (unlisten) unlisten();
    if (hideTimer !== null) clearTimeout(hideTimer);
  });

  function fmtBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  function fmtDuration(ms: number): string {
    const s = Math.floor(ms / 1000);
    if (s < 60) return `${s}s`;
    const m = Math.floor(s / 60);
    return `${m}m ${s % 60}s`;
  }

  const percent = $derived(
    progress && progress.bytesTotal > 0
      ? Math.min(100, Math.round((progress.bytesDone / progress.bytesTotal) * 100))
      : progress?.phase === 'done'
        ? 100
        : 0
  );

  const eta = $derived.by(() => {
    if (!progress || !startedAt) return null;
    if (progress.bytesDone === 0 || progress.bytesTotal === 0) return null;
    const elapsed = Date.now() - startedAt;
    const rate = progress.bytesDone / elapsed; // bytes per ms
    const remaining = progress.bytesTotal - progress.bytesDone;
    if (rate <= 0) return null;
    return remaining / rate;
  });

  const phaseLabel = $derived(
    progress?.phase === 'hashing'
      ? 'Hashing files…'
      : progress?.phase === 'uploading'
        ? 'Uploading'
        : progress?.phase === 'downloading'
          ? 'Downloading'
          : progress?.phase === 'committing'
            ? progress.currentFile?.startsWith('deploying')
              ? 'Deploying'
              : 'Committing…'
            : progress?.phase === 'done'
              ? 'Done'
              : ''
  );
</script>

{#if progress}
  <div
    class="fixed bottom-4 right-4 z-50 w-[360px]"
    role="status"
    aria-live="polite"
  >
    <Card.Root class="border-primary/25 shadow-lg">
      <Card.Content class="flex flex-col gap-3 py-4">
        <div class="flex items-center justify-between gap-2">
          <div class="flex items-center gap-2">
            {#if progress.phase === 'done'}
              <CheckCircle2Icon class="h-4 w-4 text-emerald-500" />
            {:else if progress.phase === 'downloading'}
              <DownloadCloudIcon class="h-4 w-4" />
            {:else if progress.phase === 'hashing'}
              <HashIcon class="h-4 w-4" />
            {:else}
              <UploadIcon class="h-4 w-4" />
            {/if}
            <span class="text-sm font-medium">{phaseLabel}</span>
            <Badge variant="secondary" class="font-mono-tab text-xs">
              {progress.gameSlug}
            </Badge>
          </div>
          <span class="font-mono-tab text-xs text-muted-foreground">{percent}%</span>
        </div>

        <div class="h-2 w-full overflow-hidden rounded-full bg-muted">
          <div
            class="h-full bg-primary transition-all"
            style="width: {percent}%"
          ></div>
        </div>

        <div class="flex flex-col gap-0.5 text-xs text-muted-foreground">
          {#if progress.currentFile}
            <div class="truncate font-mono-tab" title={progress.currentFile}>
              {progress.currentFile}
            </div>
          {/if}
          <div class="flex items-center justify-between font-mono-tab">
            <span>
              {fmtBytes(progress.bytesDone)} / {fmtBytes(progress.bytesTotal)}
              {#if progress.fileCount > 0}
                · file {Math.min(progress.fileIndex + 1, progress.fileCount)}/{progress.fileCount}
              {/if}
            </span>
            {#if eta !== null && progress.phase !== 'done'}
              <span>ETA {fmtDuration(eta)}</span>
            {/if}
          </div>
        </div>
      </Card.Content>
    </Card.Root>
  </div>
{/if}
