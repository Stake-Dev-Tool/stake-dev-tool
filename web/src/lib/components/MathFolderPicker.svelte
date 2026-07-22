<script lang="ts">
  /**
   * MathFolderPicker — a drop zone + click-to-browse for a math revision folder.
   *
   * Accepts a dragged folder (via `DataTransferItem.webkitGetAsEntry`, walked
   * recursively) and a `<input type="file" webkitdirectory>` browse. Produces
   * `IntakeFile[]` with relative POSIX paths (top folder stripped), skipping
   * dotfiles, and validates the house rules (a root `index.json`, ≤ 1000 files,
   * non-empty) before emitting. Nothing is read into memory here — only File
   * handles are collected; hashing/streaming happens later in `push.ts`.
   */
  import { humanSize } from '$lib/format';
  import { hasRootIndex, type IntakeFile } from '$lib/push';

  type Props = {
    disabled?: boolean;
    /**
     * The file the folder must contain at its root. Default `index.json` (math);
     * front bundles pass `index.html`. Pass the sentinel `'detect'` to accept a
     * folder that has EITHER at its root and report which kind(s) it is — the
     * per-kind file caps (math 1000, front 2000) then apply after detection.
     */
    requiredRootFile?: string;
    /** Max files allowed in the folder. Default 1000 (math); front bundles pass 2000. Ignored in `detect` mode (per-kind caps apply). */
    maxFiles?: number;
    /** Bold call-to-action in the drop zone. Default targets math folders. */
    label?: string;
    /**
     * Fired with the accepted files and the detected top folder name (may be '').
     * In `detect` mode the third argument reports which push kinds the folder is
     * valid for (present at root AND within that kind's file cap).
     */
    onpicked?: (
      files: IntakeFile[],
      rootName: string,
      roots?: { math: boolean; front: boolean }
    ) => void;
    /** Fired when the selection is cleared or rejected. */
    oncleared?: () => void;
  };

  let {
    disabled = false,
    requiredRootFile = 'index.json',
    maxFiles = 1000,
    label = 'Drop your math folder here',
    onpicked,
    oncleared
  }: Props = $props();

  // Per-kind file caps, applied after auto-detection in `detect` mode.
  const MATH_CAP = 1000;
  const FRONT_CAP = 2000;

  let detect = $derived(requiredRootFile === 'detect');
  let MAX_FILES = $derived(maxFiles);

  let dragOver = $state(false);
  let error = $state('');
  let files = $state<IntakeFile[]>([]);
  let rootName = $state('');
  let inputEl: HTMLInputElement | undefined = $state();

  let totalSize = $derived(files.reduce((a, f) => a + f.file.size, 0));
  let largest = $derived(
    files.reduce<IntakeFile | null>((m, f) => (!m || f.file.size > m.file.size ? f : m), null)
  );

  // --- FileSystemEntry helpers (Promise wrappers over the callback API) -------
  function readEntries(reader: FileSystemDirectoryReader): Promise<FileSystemEntry[]> {
    return new Promise((resolve, reject) => reader.readEntries(resolve, reject));
  }
  function entryToFile(entry: FileSystemFileEntry): Promise<File> {
    return new Promise((resolve, reject) => entry.file(resolve, reject));
  }

  async function walkEntry(entry: FileSystemEntry, prefix: string, out: IntakeFile[]): Promise<void> {
    if (entry.isFile) {
      const file = await entryToFile(entry as FileSystemFileEntry);
      out.push({ path: prefix + entry.name, file });
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      // readEntries yields in batches; keep reading until it returns none.
      let batch: FileSystemEntry[];
      do {
        batch = await readEntries(reader);
        for (const child of batch) await walkEntry(child, prefix + entry.name + '/', out);
      } while (batch.length > 0);
    }
  }

  async function collectFromEntries(entries: FileSystemEntry[]): Promise<IntakeFile[]> {
    const out: IntakeFile[] = [];
    if (entries.length === 1 && entries[0].isDirectory) {
      // Single dropped folder: strip its name by walking its children at root.
      const reader = (entries[0] as FileSystemDirectoryEntry).createReader();
      let batch: FileSystemEntry[];
      do {
        batch = await readEntries(reader);
        for (const child of batch) await walkEntry(child, '', out);
      } while (batch.length > 0);
    } else {
      for (const entry of entries) await walkEntry(entry, '', out);
    }
    return out;
  }

  // --- Path normalization + validation ---------------------------------------
  function stripTop(rel: string): string {
    const parts = rel.split('/');
    return parts.length > 1 ? parts.slice(1).join('/') : rel;
  }
  function topSegment(rel: string): string {
    return rel.split('/')[0] ?? '';
  }

  function reject(msg: string) {
    error = msg;
    files = [];
    rootName = '';
    oncleared?.();
  }

  function accept(raw: IntakeFile[], root: string) {
    error = '';
    // Normalize: forward slashes, no leading slash, drop dotfiles/dot-dirs.
    const byPath = new Map<string, IntakeFile>();
    for (const item of raw) {
      const p = item.path.replace(/\\/g, '/').replace(/^\/+/, '');
      if (!p) continue;
      const segs = p.split('/');
      if (segs.some((s) => s.startsWith('.'))) continue; // skip dotfiles & dot-dirs
      byPath.set(p, { path: p, file: item.file });
    }
    const finalFiles = [...byPath.values()];

    if (finalFiles.length === 0) {
      reject('That folder has no usable files (dotfiles are skipped).');
      return;
    }

    // --- Auto-detect mode: accept index.json (math) OR index.html (front) ------
    if (detect) {
      const paths = finalFiles.map((f) => f.path);
      const hasMath = hasRootIndex(paths, 'index.json');
      const hasFront = hasRootIndex(paths, 'index.html');
      if (!hasMath && !hasFront) {
        reject(
          'No index.json or index.html at the folder root. Drop a math folder (index.json) or a front build (index.html).'
        );
        return;
      }
      // Per-kind caps: a kind is only available if its root file is present AND
      // the folder is within that kind's cap. If a folder has both roots but
      // exceeds one cap, that kind drops out while the other stays valid.
      const mathAvailable = hasMath && finalFiles.length <= MATH_CAP;
      const frontAvailable = hasFront && finalFiles.length <= FRONT_CAP;
      if (!mathAvailable && !frontAvailable) {
        const cap = hasFront ? FRONT_CAP : MATH_CAP;
        reject(`Too many files: ${finalFiles.length.toLocaleString()} (max ${cap.toLocaleString()}).`);
        return;
      }
      files = finalFiles;
      rootName = root;
      onpicked?.(finalFiles, root, { math: mathAvailable, front: frontAvailable });
      return;
    }

    if (finalFiles.length > MAX_FILES) {
      reject(`Too many files: ${finalFiles.length.toLocaleString()} (max ${MAX_FILES.toLocaleString()}).`);
      return;
    }
    if (!hasRootIndex(finalFiles.map((f) => f.path), requiredRootFile)) {
      reject(`No ${requiredRootFile} at the folder root. One is required there.`);
      return;
    }

    files = finalFiles;
    rootName = root;
    onpicked?.(finalFiles, root);
  }

  // --- Event handlers ---------------------------------------------------------
  async function onDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    if (disabled) return;
    const dt = e.dataTransfer;
    if (!dt) return;

    // Capture entries synchronously — the DataTransfer is only valid during the event.
    const items = Array.from(dt.items).filter((it) => it.kind === 'file');
    const entries = items
      .map((it) => it.webkitGetAsEntry?.() ?? null)
      .filter((x): x is FileSystemEntry => x != null);
    const droppedFiles = Array.from(dt.files);

    try {
      if (entries.length > 0) {
        const root = entries.length === 1 && entries[0].isDirectory ? entries[0].name : '';
        accept(await collectFromEntries(entries), root);
      } else if (droppedFiles.length > 0) {
        // No directory entries (plain files) — use bare names as paths.
        accept(droppedFiles.map((f) => ({ path: f.name, file: f })), '');
      } else {
        reject('Nothing to read from that drop. Try the browse button.');
      }
    } catch {
      reject('Could not read that folder. Try the browse button instead.');
    }
  }

  function onDragOver(e: DragEvent) {
    if (disabled) return;
    e.preventDefault();
    dragOver = true;
  }
  function onDragLeave(e: DragEvent) {
    if (e.currentTarget === e.target) dragOver = false;
  }

  function onBrowse(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const list = input.files ? Array.from(input.files) : [];
    if (list.length === 0) return;
    // webkitRelativePath is "top/sub/file"; strip the top folder for the manifest.
    const root = topSegment(list[0].webkitRelativePath || '');
    accept(
      list.map((f) => ({ path: stripTop(f.webkitRelativePath || f.name), file: f })),
      root
    );
    input.value = ''; // allow re-picking the same folder
  }

  /** Set the non-standard directory-picker attributes without tripping TS types. */
  function directoryInput(node: HTMLInputElement) {
    node.setAttribute('webkitdirectory', '');
    node.setAttribute('directory', '');
  }

  export function clear() {
    reject('');
    error = '';
  }
</script>

<div class="flex flex-col gap-3">
  <div
    role="button"
    tabindex="0"
    aria-disabled={disabled}
    class="flex flex-col items-center justify-center gap-2 rounded-lg border border-dashed px-6 py-8 text-center transition
      {dragOver ? 'border-accent/70 bg-accent/5' : 'border-border bg-surface-2/40'}
      {disabled ? 'pointer-events-none opacity-50' : ''}"
    ondrop={onDrop}
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    onclick={() => inputEl?.click()}
    onkeydown={(e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        inputEl?.click();
      }
    }}
  >
    <span class="flex h-10 w-10 items-center justify-center rounded-xl bg-accent/10 text-accent">
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 7a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
        <path d="M12 11v5M9.5 13.5 12 11l2.5 2.5" />
      </svg>
    </span>
    <div class="text-sm">
      <span class="font-medium text-text">{label}</span>
      <span class="text-muted"> or </span>
      <span class="text-accent underline-offset-2 hover:underline">browse</span>
    </div>
    {#if detect}
      <p class="text-xs text-faint">
        Root <span class="font-mono-tab text-muted">index.json</span> (math, up to {MATH_CAP.toLocaleString()} files)
        or <span class="font-mono-tab text-muted">index.html</span> (front, up to {FRONT_CAP.toLocaleString()} files)
      </p>
    {:else}
      <p class="text-xs text-faint">
        The folder must contain an <span class="font-mono-tab text-muted">{requiredRootFile}</span> at its root · up to {MAX_FILES.toLocaleString()} files
      </p>
    {/if}
    <input
      bind:this={inputEl}
      use:directoryInput
      type="file"
      multiple
      class="hidden"
      onchange={onBrowse}
      {disabled}
    />
  </div>

  {#if error}
    <p class="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">{error}</p>
  {/if}

  {#if files.length > 0}
    <div class="fade-in flex flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-surface-2/50 px-4 py-3 text-sm">
      <div class="flex flex-wrap items-center gap-x-4 gap-y-1">
        <span><span class="font-mono-tab font-semibold text-text">{files.length.toLocaleString()}</span> <span class="text-muted">files</span></span>
        <span class="font-mono-tab text-muted">{humanSize(totalSize)}</span>
        {#if largest}
          <span class="text-faint">largest <span class="font-mono-tab">{largest.path}</span> ({humanSize(largest.file.size)})</span>
        {/if}
      </div>
      {#if !disabled}
        <button type="button" class="text-xs text-muted transition hover:text-text" onclick={() => clear()}>
          Choose a different folder
        </button>
      {/if}
    </div>
  {/if}
</div>
