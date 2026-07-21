//! Object store → local disk materialization for the multi-tenant LGS host.
//!
//! The LGS engine reads math from disk (mmap for books), so a revision's files
//! must be materialized into a local directory before it can serve play. This
//! generalizes the small-file pattern in [`crate::stats`] to stream *every* file
//! of a revision (books included) to disk, verifying sha256 on the way.
//!
//! ## Layout
//! Files land under `<cache_root>/rev/<workspace>/<game>/<number>/<game_slug>/…`.
//! The `<number>` directory is the tenant's math root: `DiskMathSource` resolves
//! `<number>/<game_slug>/<file>`, so the extra `<game_slug>/` level is required.
//!
//! ## Idempotence & crash-safety
//! A `.complete` marker file is written *last*. Its presence short-circuits
//! re-materialization; its absence (a crash mid-download, or a manually deleted
//! cache dir) causes the partial directory to be wiped and rebuilt. A
//! per-revision async mutex means concurrent first requests download exactly
//! once.
//!
//! ## Eviction
//! Completed revision directories are evicted LRU by the marker's mtime once
//! their combined size exceeds the byte budget (`SERVER_MATH_CACHE_BYTES`).
//! Directories still materializing have no marker and are therefore never
//! considered; the just-materialized directory is explicitly kept. Eviction is
//! best-effort — a directory whose files are momentarily held open (e.g. a
//! concurrent decompression on Windows) is logged and skipped, not fatal.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use futures_util::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::blobs;

/// Marker file written once a revision directory is fully materialized. Its
/// mtime doubles as the LRU recency signal.
const MARKER: &str = ".complete";

pub(super) struct Materializer {
    cache_root: PathBuf,
    budget_bytes: u64,
    /// Per-revision (keyed by the number directory) async locks so concurrent
    /// first requests wait on a single download.
    locks: DashMap<PathBuf, Arc<Mutex<()>>>,
}

impl Materializer {
    pub(super) fn new(cache_root: PathBuf, budget_bytes: u64) -> Self {
        Self {
            cache_root,
            budget_bytes,
            locks: DashMap::new(),
        }
    }

    /// The tenant's math root — the `<number>` directory whose child
    /// `<game_slug>/` holds the materialized files.
    fn number_dir(&self, workspace_id: Uuid, game_id: Uuid, number: i32) -> PathBuf {
        self.cache_root
            .join("rev")
            .join(workspace_id.to_string())
            .join(game_id.to_string())
            .join(number.to_string())
    }

    /// Ensure the revision is materialized on disk, returning its math root
    /// (`<number>` directory). Idempotent: a completed directory is reused (and
    /// its marker touched for LRU recency).
    pub(super) async fn ensure(
        &self,
        store: &dyn ObjectStore,
        pool: &PgPool,
        rev: &super::RevisionRef<'_>,
    ) -> anyhow::Result<PathBuf> {
        let number_dir = self.number_dir(rev.workspace_id, rev.game_id, rev.number);
        let marker = number_dir.join(MARKER);

        // Fast path: already complete. Refresh recency and return.
        if path_exists(&marker).await {
            touch(&marker).await;
            return Ok(number_dir);
        }

        // Serialize concurrent first requests for this exact revision.
        let lock = self.lock_for(&number_dir);
        let _guard = lock.lock().await;

        // Re-check under the lock — another task may have just finished.
        if path_exists(&marker).await {
            touch(&marker).await;
            return Ok(number_dir);
        }

        // A partial directory (present without a marker) is a crashed or evicted
        // attempt: wipe it and rebuild from scratch.
        if path_exists(&number_dir).await {
            tokio::fs::remove_dir_all(&number_dir).await.ok();
        }
        let game_dir = number_dir.join(rev.game_slug);
        tokio::fs::create_dir_all(&game_dir).await?;

        // The revision's manifest: (path, sha256) for every file.
        let files: Vec<(String, Vec<u8>)> =
            sqlx::query_as("SELECT path, hash FROM revision_files WHERE revision_id = $1")
                .bind(rev.revision_id)
                .fetch_all(pool)
                .await?;
        if files.is_empty() {
            anyhow::bail!("revision {} has no files to materialize", rev.revision_id);
        }

        for (path, hash) in &files {
            let hash_hex = blobs::to_hex(hash);
            let dest = game_dir.join(path);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            self.stream_verify(store, rev.workspace_id, &hash_hex, &dest)
                .await?;
        }

        // Marker last: a crash before this leaves a partial dir that the next
        // request re-materializes.
        tokio::fs::write(&marker, now_millis().to_string()).await?;

        // Enforce the on-disk byte budget, never evicting what we just wrote.
        self.evict_if_needed(&number_dir).await;

        Ok(number_dir)
    }

    /// Stream one blob from the object store to `dest`, verifying its sha256
    /// while writing (the store is content-addressed, so a mismatch means store
    /// corruption).
    async fn stream_verify(
        &self,
        store: &dyn ObjectStore,
        workspace_id: Uuid,
        hash_hex: &str,
        dest: &Path,
    ) -> anyhow::Result<()> {
        let key = blobs::blob_key(workspace_id, hash_hex);
        let result = store.get(&key).await?;
        let mut stream = result.into_stream();
        let mut file = tokio::fs::File::create(dest).await?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        let got = blobs::to_hex(hasher.finalize().as_slice());
        if got != hash_hex {
            anyhow::bail!(
                "sha256 mismatch materializing {} (want {hash_hex}, got {got})",
                dest.display()
            );
        }
        Ok(())
    }

    fn lock_for(&self, number_dir: &Path) -> Arc<Mutex<()>> {
        self.locks
            .entry(number_dir.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Evict completed revision directories LRU by marker mtime until the total
    /// materialized size fits the budget. Runs on the blocking pool (filesystem
    /// walk + deletes) and never evicts `keep`.
    async fn evict_if_needed(&self, keep: &Path) {
        let cache_root = self.cache_root.clone();
        let budget = self.budget_bytes;
        let keep = keep.to_path_buf();
        let _ =
            tokio::task::spawn_blocking(move || evict_blocking(&cache_root, budget, &keep)).await;
    }
}

struct CompletedDir {
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
}

fn evict_blocking(cache_root: &Path, budget: u64, keep: &Path) {
    let mut dirs = scan_completed(cache_root);
    let mut total: u64 = dirs.iter().map(|d| d.size).sum();
    if total <= budget {
        return;
    }
    // Oldest (smallest mtime) first.
    dirs.sort_by_key(|d| d.mtime);
    for d in dirs {
        if total <= budget {
            break;
        }
        if d.path == keep {
            continue;
        }
        match std::fs::remove_dir_all(&d.path) {
            Ok(()) => {
                total = total.saturating_sub(d.size);
                tracing::info!(
                    dir = %d.path.display(),
                    bytes = d.size,
                    "evicted materialized revision (LRU)"
                );
            }
            Err(e) => {
                tracing::warn!(
                    dir = %d.path.display(),
                    error = %e,
                    "failed to evict materialized revision (still in use?)"
                );
            }
        }
    }
}

/// Collect every completed revision directory under `<cache_root>/rev/` — i.e.
/// `rev/<ws>/<game>/<number>/` directories that contain a `.complete` marker.
fn scan_completed(cache_root: &Path) -> Vec<CompletedDir> {
    let mut out = Vec::new();
    let rev_root = cache_root.join("rev");
    let Ok(ws_iter) = std::fs::read_dir(&rev_root) else {
        return out;
    };
    for ws in ws_iter.flatten() {
        let Ok(game_iter) = std::fs::read_dir(ws.path()) else {
            continue;
        };
        for game in game_iter.flatten() {
            let Ok(num_iter) = std::fs::read_dir(game.path()) else {
                continue;
            };
            for num in num_iter.flatten() {
                let dir = num.path();
                let Ok(meta) = std::fs::metadata(dir.join(MARKER)) else {
                    continue; // no marker → still materializing or not a rev dir
                };
                let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
                out.push(CompletedDir {
                    path: dir.clone(),
                    size: dir_size(&dir),
                    mtime,
                });
            }
        }
    }
    out
}

/// Total size of every regular file under `dir` (iterative, no recursion depth
/// limit).
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(iter) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in iter.flatten() {
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(entry.path()),
                Ok(ft) if ft.is_file() => {
                    if let Ok(meta) = entry.metadata() {
                        total += meta.len();
                    }
                }
                _ => {}
            }
        }
    }
    total
}

async fn path_exists(p: &Path) -> bool {
    tokio::fs::metadata(p).await.is_ok()
}

/// Refresh a marker's mtime (LRU recency) by rewriting it. A failure only costs
/// a redundant re-materialization later, so it is best-effort.
async fn touch(marker: &Path) {
    if let Err(e) = tokio::fs::write(marker, now_millis().to_string()).await {
        tracing::warn!(marker = %marker.display(), error = %e, "failed to touch .complete marker");
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
