<script lang="ts">
  /**
   * PushPanel — the game page's single "Push" action. One folder picker
   * auto-detects what is being pushed from the dropped folder's root:
   *
   *   • `index.json` at root  → a math revision (message required; commit creates
   *     a revision, then the parent navigates to it).
   *   • `index.html` at root  → a front bundle (no message; success shows a toast,
   *     no navigation — shares and the test view use the latest bundle).
   *   • BOTH present           → a small radio lets the user choose which to push.
   *   • NEITHER                → the picker rejects with a clear error naming both.
   *
   * It reuses the exact push internals of the two flows it replaces: the math
   * per-file progress table (from MathPushPanel) and the front compact recap +
   * success toast (from FrontPushPanel), both driven by `runPush`/`runFrontPush`.
   */
  import { isUpgradeError } from '$lib/api';
  import { humanSize } from '$lib/format';
  import { toast } from '$lib/toasts.svelte';
  import {
    runPush,
    runFrontPush,
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

  type Kind = 'math' | 'front';

  type Props = {
    slug: string;
    game: string;
    /** The game's current head number (parent). Null when it has no revisions. */
    parentNumber?: number | null;
    /** Fired with the new revision number after a successful math push. */
    ondone?: (revisionNumber: number) => void;
    /** Fired with the new bundle id after a successful front push. */
    onfrontuploaded?: (bundleId: string) => void;
    oncancel?: () => void;
  };

  let { slug, game, parentNumber = null, ondone, onfrontuploaded, oncancel }: Props = $props();

  const MAX_ROWS = 250; // cap rendered per-file rows; the recap covers the totals

  // --- Selection + detection --------------------------------------------------
  let pickerKey = $state(0); // bump to remount the picker (clears its selection)
  let pickedFiles = $state<IntakeFile[]>([]);
  // Which kinds the picked folder is valid for (present at root + within cap).
  let available = $state<{ math: boolean; front: boolean }>({ math: false, front: false });
  // The kind to push. Auto-set from detection; user-switchable only when both fit.
  let kind = $state<Kind | null>(null);
  let bothAvailable = $derived(available.math && available.front);

  let message = $state('');

  // --- Push run state ---------------------------------------------------------
  let pushing = $state(false);
  let pushError = $state('');
  // True when `pushError` is a billing gate (expired trial / quota) — the panel
  // then shows an inline "Upgrade →" link instead of a plain error.
  let pushErrorUpgrade = $state(false);
  let phase = $state<PushPhase | null>(null);
  let progress = $state<FileProgress[]>([]);

  let nextNumber = $derived((parentNumber ?? 0) + 1);
  let hasSelection = $derived(pickedFiles.length > 0 && kind != null);

  let canPush = $derived(
    hasSelection &&
      !pushing &&
      (kind === 'front' || message.trim().length > 0)
  );

  let primaryLabel = $derived(
    kind === 'front' ? 'Push front bundle' : `Push revision ${nextNumber}`
  );

  // --- Progress recap (derived from per-file statuses) ------------------------
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
        return kind === 'front' ? 'Committing front bundle…' : 'Committing revision…';
      case 'done':
        return kind === 'front' ? 'Done.' : 'Done — opening the revision…';
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

  // --- Picker callbacks -------------------------------------------------------
  function onPicked(files: IntakeFile[], _root: string, roots?: { math: boolean; front: boolean }) {
    pickedFiles = files;
    available = roots ?? { math: false, front: false };
    // Auto-select: a single valid kind is chosen for the user; when both fit,
    // default to a math revision (the primary action) but expose the radio.
    kind = available.math ? 'math' : available.front ? 'front' : null;
    pushError = '';
  }
  function onCleared() {
    pickedFiles = [];
    available = { math: false, front: false };
    kind = null;
  }
  function resetSelection() {
    onCleared();
    message = '';
    phase = null;
    progress = [];
    pushError = '';
    pushErrorUpgrade = false;
    pickerKey += 1; // remount the picker to clear its own selection
  }

  // --- Push -------------------------------------------------------------------
  async function doPush() {
    if (!canPush || kind == null) return;
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
    const hooks = {
      onPhase: (p: PushPhase) => (phase = p),
      onFileUpdate: (i: number, patch: Partial<FileProgress>) => {
        const cur = progress[i];
        if (cur) progress[i] = { ...cur, ...patch };
      }
    };
    try {
      if (kind === 'front') {
        const res = await runFrontPush({ slug, game, files: pickedFiles }, hooks);
        onfrontuploaded?.(res.bundleId);
        toast.success('Front bundle uploaded — new shares and the test view use it automatically.');
        resetSelection();
      } else {
        const res = await runPush(
          { slug, game, message: message.trim(), parentNumber, files: pickedFiles },
          hooks
        );
        ondone?.(res.number);
      }
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
      <h2 class="text-base font-semibold">Push</h2>
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

    {#key pickerKey}
      <MathFolderPicker
        disabled={pushing}
        requiredRootFile="detect"
        label="Drop your math folder or front build here"
        onpicked={onPicked}
        oncleared={onCleared}
      />
    {/key}

    {#if hasSelection}
      {#if bothAvailable}
        <!-- Both roots present: let the user choose what to push. -->
        <fieldset class="flex flex-col gap-2 rounded-md border border-border bg-surface-2/40 p-4">
          <legend class="px-1 text-xs font-medium text-muted">
            This folder has both — choose what to push
          </legend>
          <label class="flex cursor-pointer items-center gap-2.5 text-sm">
            <input type="radio" value="math" bind:group={kind} disabled={pushing} class="accent-accent" />
            <span class="text-text">Math revision</span>
            <span class="font-mono-tab text-xs text-faint">index.json</span>
          </label>
          <label class="flex cursor-pointer items-center gap-2.5 text-sm">
            <input type="radio" value="front" bind:group={kind} disabled={pushing} class="accent-accent" />
            <span class="text-text">Front bundle</span>
            <span class="font-mono-tab text-xs text-faint">index.html</span>
          </label>
        </fieldset>
      {:else}
        <!-- One kind detected: show a badge naming it. -->
        <div class="flex items-center gap-2 rounded-md border border-accent/30 bg-accent/5 px-3 py-2 text-sm">
          <span class="h-1.5 w-1.5 rounded-full bg-accent"></span>
          {#if kind === 'front'}
            <span class="font-medium text-text">Front bundle</span>
            <span class="text-muted">— <span class="font-mono-tab">index.html</span> found</span>
          {:else}
            <span class="font-medium text-text">Math revision</span>
            <span class="text-muted">— <span class="font-mono-tab">index.json</span> found</span>
          {/if}
        </div>
      {/if}
    {/if}

    {#if hasSelection && kind === 'math'}
      <Input
        id="push-message"
        label="Message"
        bind:value={message}
        placeholder="Describe what changed in this revision"
        disabled={pushing}
        hint="Required — shown in the revision list."
      />
    {/if}

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

        <!-- Per-file states: only for math (a front build is many small files). -->
        {#if kind === 'math'}
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
        {/if}
      </div>
    {/if}

    <div class="flex flex-wrap items-center gap-x-4 gap-y-2">
      <Button type="submit" loading={pushing} disabled={!canPush}>
        {primaryLabel}
      </Button>
      <span class="text-xs text-faint">
        Prefer CI? <span class="font-mono-tab text-muted">sdt push</span> /
        <span class="font-mono-tab text-muted">sdt push-front</span> still work from your pipeline.
      </span>
    </div>
  </form>
</Card>
