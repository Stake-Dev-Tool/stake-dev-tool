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
    /** Fired with the accepted files and the detected top folder name (may be ''). */
    onpicked?: (files: IntakeFile[], rootName: string) => void;
    /** Fired when the selection is cleared or rejected. */
    oncleared?: () => void;
  };

  let { disabled = false, onpicked, oncleared }: Props = $props();

  const MAX_FILES = 1000;

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
    if (finalFiles.length > MAX_FILES) {
      reject(`Too many files: ${finalFiles.length.toLocaleString()} (max ${MAX_FILES.toLocaleString()}).`);
      return;
    }
    if (!hasRootIndex(finalFiles.map((f) => f.path))) {
      reject('No index.json at the folder root. A math revision needs one there.');
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
      <span class="font-medium text-text">Drop your math folder here</span>
      <span class="text-muted"> or </span>
      <span class="text-accent underline-offset-2 hover:underline">browse</span>
    </div>
    <p class="text-xs text-faint">
      The folder must contain an <span class="font-mono-tab text-muted">index.json</span> at its root · up to {MAX_FILES.toLocaleString()} files
    </p>
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
