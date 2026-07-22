<script lang="ts">
  /**
   * MathPushPanel — the browser "push a revision" flow, shared by the game page
   * (push into an existing game) and the workspace Games card (create a new game).
   *
   * Wraps MathFolderPicker + a required message + (for a new game) a live-derived,
   * validated game-slug input, then runs the `push.ts` pipeline and renders its
   * per-file + global progress. On success it hands the new revision number back
   * to the parent, which navigates to the revision page (where stats poll).
   */
  import { isValidSlug, isUpgradeError, slugFromName } from '$lib/api';
  import { humanSize } from '$lib/format';
  import {
    runPush,
    pushErrorMessage,
    type FileProgress,
    type PushPhase,
    type IntakeFile
  } from '$lib/push';
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';
  import Card from '$lib/components/Card.svelte';
  import MathFolderPicker from '$lib/components/MathFolderPicker.svelte';
  import UpgradeNotice from '$lib/components/UpgradeNotice.svelte';

  type Props = {
    slug: string;
    /** Existing game slug, or null for the "new game" flow (shows a slug input). */
    game?: string | null;
    /** The game's current head number (parent). Null for a new game. */
    parentNumber?: number | null;
    ondone?: (revNumber: number, gameSlug: string) => void;
    oncancel?: () => void;
  };

  let { slug, game = null, parentNumber = null, ondone, oncancel }: Props = $props();

  const MAX_ROWS = 250; // cap rendered per-file rows; the recap covers the totals

  let isNew = $derived(game == null);

  let pickedFiles = $state<IntakeFile[]>([]);
  let rootName = $state('');
  let message = $state('');

  // New-game slug, live-derived from the picked folder name until edited.
  let gameSlug = $state('');
  let gameSlugEdited = $state(false);
  $effect(() => {
    if (isNew && !gameSlugEdited && rootName) gameSlug = slugFromName(rootName);
  });
  let slugInvalid = $derived(isNew && gameSlug.length > 0 && !isValidSlug(gameSlug));

  let pushing = $state(false);
  let pushError = $state('');
  // True when `pushError` is a billing gate (no active plan / quota) — the panel
  // then shows an inline "Upgrade →" link instead of a plain error.
  let pushErrorUpgrade = $state(false);
  let phase = $state<PushPhase | null>(null);
  let progress = $state<FileProgress[]>([]);

  let canPush = $derived(
    pickedFiles.length > 0 &&
      message.trim().length > 0 &&
      (!isNew || isValidSlug(gameSlug)) &&
      !pushing
  );

  // --- Progress recap (derived from per-file statuses) -----------------------
  let total = $derived(progress.length);
  let hashedCount = $derived(
    progress.filter((p) => p.status !== 'pending' && p.status !== 'hashing').length
  );
  let processed = $derived(
    progress.filter((p) => p.status === 'uploaded' || p.status === 'deduplicated').length
  );
  let sentBytes = $derived(
    progress.filter((p) => p.status === 'uploaded').reduce((a, p) => a + p.size, 0)
  );
  let dedupCount = $derived(progress.filter((p) => p.status === 'deduplicated').length);
  let visibleProgress = $derived(progress.slice(0, MAX_ROWS));
  let hiddenCount = $derived(Math.max(0, progress.length - MAX_ROWS));

  let phaseLabel = $derived.by(() => {
    switch (phase) {
      case 'hashing':
        return `Hashing files… ${hashedCount} / ${total}`;
      case 'checking':
        return 'Checking which files the server already has…';
      case 'uploading':
        return `Uploading… ${processed} / ${total} files · ${humanSize(sentBytes)} sent · ${dedupCount} deduplicated`;
      case 'committing':
        return 'Committing revision…';
      case 'done':
        return 'Done — opening the revision…';
      default:
        return '';
    }
  });

  function statusView(p: FileProgress): { text: string; cls: string; spin: boolean } {
    switch (p.status) {
      case 'hashing': {
        const pct = p.size > 0 ? Math.round((p.hashedBytes / p.size) * 100) : 100;
        return { text: `hashing ${pct}%`, cls: 'text-muted', spin: true };
      }
      case 'hashed':
        return { text: 'hashed', cls: 'text-faint', spin: false };
      case 'uploading':
        return { text: 'uploading', cls: 'text-muted', spin: true };
      case 'uploaded':
        return { text: 'uploaded', cls: 'text-accent', spin: false };
      case 'deduplicated':
        return { text: 'deduplicated', cls: 'text-muted', spin: false };
      case 'error':
        return { text: 'failed', cls: 'text-danger', spin: false };
      default:
        return { text: 'queued', cls: 'text-faint', spin: false };
    }
  }

  function onPicked(files: IntakeFile[], root: string) {
    pickedFiles = files;
    rootName = root;
    pushError = '';
  }
  function onCleared() {
    pickedFiles = [];
    rootName = '';
  }

  async function doPush() {
    if (!canPush) return;
    const targetGame = isNew ? gameSlug.trim() : (game ?? '');
    pushing = true;
    pushError = '';
    pushErrorUpgrade = false;
    phase = 'hashing';
    progress = pickedFiles.map((f) => ({
      path: f.path,
      size: f.file.size,
      status: 'pending' as const,
      hashedBytes: 0
    }));
    try {
      const res = await runPush(
        {
          slug,
          game: targetGame,
          message: message.trim(),
          parentNumber: isNew ? null : parentNumber,
          files: pickedFiles
        },
        {
          onPhase: (p) => (phase = p),
          onFileUpdate: (i, patch) => {
            const cur = progress[i];
            if (cur) progress[i] = { ...cur, ...patch };
          }
        }
      );
      ondone?.(res.number, targetGame);
    } catch (e) {
      pushError = pushErrorMessage(e);
      pushErrorUpgrade = isUpgradeError(e);
      phase = null;
    } finally {
      pushing = false;
    }
  }
</script>

<Card class="fade-in p-6">
  <form
    class="flex flex-col gap-5"
    onsubmit={(e) => {
      e.preventDefault();
      void doPush();
    }}
  >
    <div class="flex items-center justify-between gap-3">
      <h2 class="text-base font-semibold">{isNew ? 'New game' : 'Push a revision'}</h2>
      {#if oncancel}
        <button
          type="button"
          class="text-sm text-muted transition hover:text-text disabled:opacity-50"
          disabled={pushing}
          onclick={() => oncancel?.()}
        >
          Cancel
        </button>
      {/if}
    </div>

    {#if isNew}
      <Input
        id="new-game-slug"
        label="Game slug"
        bind:value={gameSlug}
        oninput={() => (gameSlugEdited = true)}
        mono
        placeholder="my-slot"
        disabled={pushing}
        error={slugInvalid ? 'Use 3–40 chars: a–z, 0–9, hyphens (not at the ends).' : undefined}
        hint="Used in URLs — /w/{slug}/g/{gameSlug || 'your-game'}"
      />
    {/if}

    <MathFolderPicker disabled={pushing} onpicked={onPicked} oncleared={onCleared} />

    <Input
      id="push-message"
      label="Message"
      bind:value={message}
      placeholder="Describe what changed in this revision"
      disabled={pushing}
      hint="Required — shown in the revision list."
    />

    {#if pushError}
      {#if pushErrorUpgrade}
        <UpgradeNotice {slug} message={pushError} />
      {:else}
        <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
          {pushError}
        </p>
      {/if}
    {/if}

    {#if phase}
      <div class="flex flex-col gap-3 rounded-md border border-border bg-surface-2/40 p-4">
        <div class="flex items-center gap-2.5 text-sm">
          {#if phase !== 'done'}<span class="spinner text-accent"></span>{/if}
          <span class="text-text">{phaseLabel}</span>
        </div>

        <!-- Global progress bar (processed / total) -->
        <div class="h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
          <div
            class="h-full rounded-full bg-accent transition-all"
            style="width: {total > 0 ? Math.round((processed / total) * 100) : 0}%"
          ></div>
        </div>

        <!-- Per-file states -->
        <div class="max-h-56 overflow-y-auto rounded-md border border-border/60">
          <table class="w-full text-xs">
            <tbody>
              {#each visibleProgress as p (p.path)}
                {@const s = statusView(p)}
                <tr class="border-b border-border/40 last:border-0">
                  <td class="max-w-0 truncate px-3 py-1.5 font-mono-tab text-muted" title={p.path}>{p.path}</td>
                  <td class="whitespace-nowrap px-3 py-1.5 text-right font-mono-tab text-faint">{humanSize(p.size)}</td>
                  <td class="whitespace-nowrap px-3 py-1.5 text-right">
                    <span class="inline-flex items-center gap-1.5 {s.cls}">
                      {#if s.spin}<span class="spinner"></span>{/if}{s.text}
                    </span>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if hiddenCount > 0}
          <p class="text-xs text-faint">+{hiddenCount.toLocaleString()} more files (tracked in the totals above)</p>
        {/if}
      </div>
    {/if}

    <div class="flex flex-wrap items-center gap-x-4 gap-y-2">
      <Button type="submit" loading={pushing} disabled={!canPush}>
        {isNew ? 'Create game & push' : 'Push revision'}
      </Button>
      <span class="text-xs text-faint">
        Prefer CI? <span class="font-mono-tab text-muted">sdt push</span> still works from your pipeline.
      </span>
    </div>
  </form>
</Card>
