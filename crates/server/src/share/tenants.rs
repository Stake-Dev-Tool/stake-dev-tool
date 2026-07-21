//! Per-deployment multi-tenant LGS host for the *public share* path.
//!
//! This mirrors [`crate::lgs_host`]'s `LgsHost` + `Materializer`: it materializes
//! a `(workspace, game, revision)` from the object store to local disk and builds
//! a cached per-tenant [`lgs`] router. It is a deliberate copy rather than a
//! reuse because `lgs_host`'s host/materializer internals are module-private and
//! this milestone's file-ownership split forbids widening them (see the M5
//! contract's "meeting only at the router registration lines"). The public bits
//! it *does* reuse — [`lgs::TenantRegistry`] (get_or_create_disk / router_for /
//! set_tenant_cap), [`crate::lgs_host::RevisionRef`], and [`crate::blobs`] — are
//! shared verbatim.
//!
//! ## Why a separate on-disk cache subtree
//! Materialized files land under `<cache_root>/share/rev/…`, NOT `lgs_host`'s
//! `<cache_root>/rev/…`. The two subsystems own independent per-revision
//! materialization locks, so pointing them at the same directory would let one's
//! "wipe partial + rebuild" race the other's in-flight download and corrupt it.
//! Isolating the subtree costs a duplicate materialization for revisions used by
//! both the workbench (M4) and a share, but keeps each subsystem crash-safe on
//! its own. Each subtree is independently LRU-bounded by
//! `SERVER_MATH_CACHE_BYTES`, so peak disk is up to 2× that budget.
//!
//! ## Session isolation
//! The share registry is entirely separate from `lgs_host`'s, so visitor wallet
//! state (namespaced `visitor:<id>` by the dispatcher) can never touch a
//! workbench session store.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use futures_util::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::AppState;
use crate::blobs;
use crate::config::{Config, StorageConfig};
use crate::lgs_host::RevisionRef;

use lgs::{TenantId, TenantRegistry};

/// Marker file written last once a revision directory is fully materialized; its
/// mtime doubles as the LRU recency signal.
const MARKER: &str = ".complete";

/// One share host per distinct cache root (one per `AppState` in production;
/// keyed so integration tests with separate temp storage stay isolated).
static HOSTS: OnceLock<DashMap<PathBuf, Arc<ShareHost>>> = OnceLock::new();

/// Resolve (creating on first use) the share host for this state's cache root.
pub(super) fn host_for(state: &AppState) -> Arc<ShareHost> {
    let cache_root = cache_root_for(&state.config);
    HOSTS
        .get_or_init(DashMap::new)
        .entry(cache_root.clone())
        .or_insert_with(|| {
            Arc::new(ShareHost::new(
                cache_root,
                state.config.server_math_cache_bytes,
                state.config.server_tenant_books_cap_bytes,
            ))
        })
        .clone()
}

/// `<STORAGE_FS_ROOT>/../cache/share` for the fs backend; `./data/cache/share`
/// for s3. Mirrors `lgs_host::cache_root_for` but under a `share/` subtree.
fn cache_root_for(config: &Config) -> PathBuf {
    let base = match &config.storage {
        StorageConfig::Fs { root } => root
            .parent()
            .map(|parent| parent.join("cache"))
            .unwrap_or_else(|| PathBuf::from("./data/cache")),
        StorageConfig::S3 { .. } => PathBuf::from("./data/cache"),
    };
    base.join("share")
}

pub(super) struct ShareHost {
    registry: TenantRegistry,
    cache_root: PathBuf,
    budget_bytes: u64,
    books_cap: Option<u64>,
    routers: DashMap<TenantId, axum::Router>,
    /// Per-revision (keyed by the `<number>` dir) async locks so concurrent first
    /// requests wait on a single download.
    locks: DashMap<PathBuf, Arc<Mutex<()>>>,
}

impl ShareHost {
    fn new(cache_root: PathBuf, budget_bytes: u64, books_cap: Option<u64>) -> Self {
        Self {
            registry: TenantRegistry::new(),
            cache_root,
            budget_bytes,
            books_cap,
            routers: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    fn tenant_id(workspace_id: Uuid, game_id: Uuid, number: i32) -> TenantId {
        TenantId::from(format!("ws:{workspace_id}:game:{game_id}:rev:{number}"))
    }

    /// Ensure the revision is materialized, (idempotently) register its tenant
    /// over the materialized math root, apply the per-tenant books cap, and
    /// return the tenant's router (built + cached on first use).
    pub(super) async fn router_for_revision(
        &self,
        store: &dyn ObjectStore,
        pool: &PgPool,
        rev: &RevisionRef<'_>,
    ) -> anyhow::Result<axum::Router> {
        let math_root = self.materialize(store, pool, rev).await?;

        let tenant = Self::tenant_id(rev.workspace_id, rev.game_id, rev.number);
        self.registry.get_or_create_disk(tenant.clone(), &math_root);
        self.registry.set_tenant_cap(&tenant, self.books_cap);

        if let Some(cached) = self.routers.get(&tenant) {
            return Ok(cached.clone());
        }
        let built = self
            .registry
            .router_for(&tenant)
            .ok_or_else(|| anyhow::anyhow!("tenant {tenant} unregistered after get_or_create"))?;
        Ok(self.routers.entry(tenant).or_insert(built).clone())
    }

    /// The tenant's math root — the `<number>` dir whose child `<game_slug>/`
    /// holds the materialized files.
    fn number_dir(&self, workspace_id: Uuid, game_id: Uuid, number: i32) -> PathBuf {
        self.cache_root
            .join("rev")
            .join(workspace_id.to_string())
            .join(game_id.to_string())
            .join(number.to_string())
    }

    fn lock_for(&self, number_dir: &Path) -> Arc<Mutex<()>> {
        self.locks
            .entry(number_dir.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Ensure the revision is materialized on disk, returning its math root.
    /// Idempotent via the `.complete` marker; concurrent first requests wait on a
    /// per-revision lock. (Copied from `lgs_host::materialize`.)
    async fn materialize(
        &self,
        store: &dyn ObjectStore,
        pool: &PgPool,
        rev: &RevisionRef<'_>,
    ) -> anyhow::Result<PathBuf> {
        let number_dir = self.number_dir(rev.workspace_id, rev.game_id, rev.number);
        let marker = number_dir.join(MARKER);

        if path_exists(&marker).await {
            touch(&marker).await;
            return Ok(number_dir);
        }

        let lock = self.lock_for(&number_dir);
        let _guard = lock.lock().await;

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
            stream_verify(store, rev.workspace_id, &hash_hex, &dest).await?;
        }

        tokio::fs::write(&marker, now_millis().to_string()).await?;
        self.evict_if_needed(&number_dir).await;
        Ok(number_dir)
    }

    async fn evict_if_needed(&self, keep: &Path) {
        let cache_root = self.cache_root.clone();
        let budget = self.budget_bytes;
        let keep = keep.to_path_buf();
        let _ =
            tokio::task::spawn_blocking(move || evict_blocking(&cache_root, budget, &keep)).await;
    }
}

/// Stream one blob from the object store to `dest`, verifying its sha256 while
/// writing.
async fn stream_verify(
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
                tracing::info!(dir = %d.path.display(), bytes = d.size, "evicted share revision (LRU)");
            }
            Err(e) => {
                tracing::warn!(dir = %d.path.display(), error = %e, "failed to evict share revision (still in use?)");
            }
        }
    }
}

/// Collect every completed revision dir under `<cache_root>/rev/`.
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
                    continue;
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

async fn touch(marker: &Path) {
    if let Err(e) = tokio::fs::write(marker, now_millis().to_string()).await {
        tracing::warn!(marker = %marker.display(), error = %e, "failed to touch share .complete marker");
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
