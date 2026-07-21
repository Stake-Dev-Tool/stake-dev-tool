/**
 * web/src/lib/push.ts
 *
 * Browser-side "push a math revision" pipeline: hash every file → ask the server
 * which blobs are missing → upload the missing blobs → commit the revision.
 *
 * UI-agnostic: `runPush` reports progress through callbacks so any component can
 * drive it. The individual steps — streaming hash, manifest building, root-index
 * validation, upload planning, and error copy — are pure/standalone and
 * unit-testable, with the network confined to the injected `api` surface.
 *
 * The whole design streams: files are hashed from `file.stream()` in chunks and
 * uploaded with the `File` as the fetch body, so a multi-GB book is never held
 * in memory in one piece.
 */
import { createSHA256, type IHasher } from 'hash-wasm';
import { api, ApiError, type RevisionFile } from './api';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** A file staged for a push. Path is relative POSIX with the top folder stripped. */
export interface IntakeFile {
  /** e.g. "index.json", "books/base.json" — forward slashes, no leading slash. */
  path: string;
  file: File;
}

/** An intake file with its computed content hash. */
export interface HashedFile {
  path: string;
  hash: string; // lowercase hex sha256
  size: number;
  file: File;
}

/** Per-file lifecycle state, surfaced to the UI. */
export type FileStatus =
  | 'pending'
  | 'hashing'
  | 'hashed'
  | 'uploading'
  | 'uploaded'
  | 'deduplicated'
  | 'error';

export interface FileProgress {
  path: string;
  size: number;
  status: FileStatus;
  /** Bytes hashed so far (drives the per-file hashing indicator). */
  hashedBytes: number;
}

/** Coarse pipeline phase, for a headline status line. */
export type PushPhase = 'hashing' | 'checking' | 'uploading' | 'committing' | 'done';

export interface PushHooks {
  onPhase?: (phase: PushPhase) => void;
  /** Fine-grained per-file updates, keyed by the file's index in the input list. */
  onFileUpdate?: (index: number, patch: Partial<FileProgress>) => void;
}

export interface PushResult {
  /** The created revision's number. */
  number: number;
  totalFiles: number;
  /** Files whose bytes we actually sent (unique missing blobs). */
  uploadedFiles: number;
  uploadedBytes: number;
  /** Files the server already had, or that duplicated another file in this push. */
  deduplicatedFiles: number;
}

// ---------------------------------------------------------------------------
// Pure helpers (no network, no DOM) — unit-testable
// ---------------------------------------------------------------------------

/** The one file a manifest must contain at its root. */
export const ROOT_INDEX = 'index.json';

/** True when the manifest has an `index.json` at its root. */
export function hasRootIndex(paths: readonly string[]): boolean {
  return paths.includes(ROOT_INDEX);
}

/** Build the wire manifest from hashed files, sorted by path for stable output. */
export function toManifest(files: readonly HashedFile[]): RevisionFile[] {
  return files
    .map((f) => ({ path: f.path, hash: f.hash, size: f.size }))
    .sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

/** One blob to upload: a representative file for a unique missing hash. */
export interface PlannedUpload {
  hash: string;
  size: number;
  file: File;
  /** Index (in the original input list) of the representative file. */
  fileIndex: number;
}

export interface UploadPlan {
  /** Unique missing hashes to PUT, one representative file each. */
  uploads: PlannedUpload[];
  /** Indices of files that need no upload (already stored, or a same-hash dup). */
  dedupedIndices: number[];
}

/**
 * Decide what to upload. A file is uploaded only if its hash is in `missing` and
 * no earlier file already claimed that hash — so identical files (same content)
 * are sent once and every other file rides the dedup, exactly like the server's
 * content-addressed store.
 */
export function planUploads(files: readonly HashedFile[], missing: Iterable<string>): UploadPlan {
  const missingSet = new Set<string>();
  for (const h of missing) missingSet.add(h.toLowerCase());

  const uploads: PlannedUpload[] = [];
  const dedupedIndices: number[] = [];
  const claimed = new Set<string>();

  files.forEach((f, i) => {
    const h = f.hash.toLowerCase();
    if (missingSet.has(h) && !claimed.has(h)) {
      claimed.add(h);
      uploads.push({ hash: h, size: f.size, file: f.file, fileIndex: i });
    } else {
      dedupedIndices.push(i);
    }
  });

  return { uploads, dedupedIndices };
}

/**
 * Map a push failure to friendly copy. Pure + testable. Covers the M2 error
 * codes: 413/`payload_too_large`, 422 `hash_mismatch`/`invalid_manifest`, 409
 * `stale_parent`/`missing_blobs`, plus network errors.
 */
export function pushErrorMessage(e: unknown): string {
  if (e instanceof ApiError) {
    switch (e.code) {
      case 'payload_too_large':
        return 'A file is larger than the server allows.';
      case 'hash_mismatch':
        return 'A file changed while uploading (hash mismatch). Re-select the folder and try again.';
      case 'invalid_manifest':
        return e.message || 'The server rejected the manifest.';
      case 'stale_parent':
        return 'Someone pushed to this game meanwhile — reload and try again.';
      case 'missing_blobs':
        return 'Some files could not be uploaded. Please try again.';
      case 'network_error':
        return 'Could not reach the server. Check your connection and try again.';
    }
    if (e.status === 413) return 'A file is larger than the server allows.';
    return e.message || 'The push failed.';
  }
  return e instanceof Error ? e.message : String(e);
}

// ---------------------------------------------------------------------------
// Streaming hash
// ---------------------------------------------------------------------------

/** Create a reusable sha256 hasher (call `hasher.init()` to reset per file). */
export async function createHasher(): Promise<IHasher> {
  return createSHA256();
}

/**
 * Stream-hash a File to a lowercase hex sha256, reading it in chunks via
 * `file.stream()` so the whole file never sits in memory. Pass a hasher from
 * `createHasher()` and reuse it across files (this resets it via `init()`) to
 * avoid re-instantiating the wasm module each time.
 */
export async function hashFile(
  file: File,
  hasher: IHasher,
  onProgress?: (bytesHashed: number) => void
): Promise<string> {
  hasher.init();
  const reader = file.stream().getReader();
  let bytes = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value && value.byteLength) {
        hasher.update(value);
        bytes += value.byteLength;
        onProgress?.(bytes);
      }
    }
  } finally {
    reader.releaseLock();
  }
  return hasher.digest('hex');
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

/** Run `worker` over `items` with at most `concurrency` in flight. */
async function mapPool<T>(
  items: readonly T[],
  concurrency: number,
  worker: (item: T, index: number) => Promise<void>
): Promise<void> {
  let next = 0;
  const lanes = Math.max(1, Math.min(concurrency, items.length));
  const runners: Promise<void>[] = [];
  for (let lane = 0; lane < lanes; lane++) {
    runners.push(
      (async () => {
        for (;;) {
          const i = next++;
          if (i >= items.length) break;
          await worker(items[i], i);
        }
      })()
    );
  }
  await Promise.all(runners);
}

/** Read the `missing` hash list off a 409 `missing_blobs` error body. */
function missingFromError(e: ApiError): string[] {
  const raw = (e.details as { missing?: unknown } | null | undefined)?.missing;
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  for (const v of raw) {
    const h = typeof v === 'string' ? v : (v as { hash?: unknown } | null)?.hash;
    if (typeof h === 'string' && h.length > 0) out.push(h.toLowerCase());
  }
  return out;
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/** Max concurrent blob uploads. Small: books are large; keep the pipe steady. */
const UPLOAD_CONCURRENCY = 3;

export interface PushInput {
  slug: string;
  /** Target game slug. For a new game this is the (implicitly created) slug. */
  game: string;
  message: string;
  /** The game's current head number, or null for a brand-new game. */
  parentNumber: number | null;
  files: IntakeFile[];
}

/**
 * Run the full push: hash → check → upload missing → commit. Reports progress
 * through `hooks`. On a 409 `missing_blobs` race at commit time it re-uploads
 * exactly the hashes the server names and retries the commit once. Throws
 * ApiError (map it with `pushErrorMessage`) on any unrecoverable failure.
 */
export async function runPush(input: PushInput, hooks: PushHooks = {}): Promise<PushResult> {
  const { slug, game, message, parentNumber, files } = input;

  let uploadedFiles = 0;
  let uploadedBytes = 0;

  const upload = async (u: PlannedUpload): Promise<void> => {
    hooks.onFileUpdate?.(u.fileIndex, { status: 'uploading' });
    try {
      await api.games.putBlob(slug, game, u.hash, u.file);
    } catch (e) {
      hooks.onFileUpdate?.(u.fileIndex, { status: 'error' });
      throw e;
    }
    uploadedFiles += 1;
    uploadedBytes += u.size;
    hooks.onFileUpdate?.(u.fileIndex, { status: 'uploaded' });
  };

  // 1) Hash every file, sequentially — only one file streams at a time.
  hooks.onPhase?.('hashing');
  const hasher = await createHasher();
  const hashed: HashedFile[] = [];
  for (let i = 0; i < files.length; i++) {
    const { path, file } = files[i];
    hooks.onFileUpdate?.(i, { status: 'hashing', hashedBytes: 0 });
    // Throttle progress to ~1% (min 1 MB) steps so a multi-GB book doesn't emit
    // tens of thousands of updates while streaming.
    let reported = 0;
    const step = Math.max(1_000_000, Math.floor(file.size / 100));
    const hash = await hashFile(file, hasher, (b) => {
      if (b - reported >= step) {
        reported = b;
        hooks.onFileUpdate?.(i, { hashedBytes: b });
      }
    });
    hashed.push({ path, hash, size: file.size, file });
    hooks.onFileUpdate?.(i, { status: 'hashed', hashedBytes: file.size });
  }
  const manifest = toManifest(hashed);

  // 2) Ask the server which blobs it still needs.
  hooks.onPhase?.('checking');
  const missing = await api.games.check(slug, game, manifest);

  // 3) Upload the missing blobs (unique by hash), a few at a time.
  const plan = planUploads(hashed, missing);
  for (const i of plan.dedupedIndices) hooks.onFileUpdate?.(i, { status: 'deduplicated' });
  if (plan.uploads.length > 0) {
    hooks.onPhase?.('uploading');
    await mapPool(plan.uploads, UPLOAD_CONCURRENCY, upload);
  }

  // 4) Commit. On a missing_blobs race, re-upload just those and retry once.
  hooks.onPhase?.('committing');
  let number: number;
  try {
    number = (await api.games.commit(slug, game, { message, files: manifest, parent_number: parentNumber })).number;
  } catch (e) {
    if (e instanceof ApiError && e.code === 'missing_blobs') {
      const retryHashes = missingFromError(e);
      if (retryHashes.length === 0) throw e;
      const retryPlan = planUploads(hashed, retryHashes);
      if (retryPlan.uploads.length > 0) {
        hooks.onPhase?.('uploading');
        await mapPool(retryPlan.uploads, UPLOAD_CONCURRENCY, upload);
      }
      hooks.onPhase?.('committing');
      number = (await api.games.commit(slug, game, { message, files: manifest, parent_number: parentNumber })).number;
    } else {
      throw e;
    }
  }

  hooks.onPhase?.('done');
  return {
    number,
    totalFiles: files.length,
    uploadedFiles,
    uploadedBytes,
    deduplicatedFiles: plan.dedupedIndices.length
  };
}
