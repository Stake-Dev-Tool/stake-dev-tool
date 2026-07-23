//! M4 — multi-tenant LGS hosting.
//!
//! Hosts many isolated `(workspace, game, revision)` math roots in one server
//! process. A request under `/api/ws/:slug/g/:game/r/:number/*rest` is
//! authenticated + membership-checked here, its revision materialized from the
//! object store to local disk ([`materialize`]), and then forwarded byte-for-
//! byte into a per-tenant [`lgs`] router ([`dispatch`]).
//!
//! ## The inner LGS is unauthenticated by design
//! `lgs::routes` / `lgs::devtool` / `lgs::replay` carry no auth of their own.
//! THIS module is the auth boundary: [`dispatch::dispatch`] resolves workspace
//! membership (404 for non-members) *before* the request ever reaches a tenant
//! router. A tenant router must never be mounted without that gate in front.
//!
//! ## Sessions & per-user isolation (deferred to M6)
//! Each tenant owns an isolated in-memory [`lgs::session`] store, so play state
//! never leaks across the tenant boundary (workspace/game/revision) — the real
//! security boundary. Per-*user* namespacing of LGS session ids within a tenant
//! is NOT enforced in M4: two teammates who pick the same client-provided
//! session id would share wallet state. That is a UX concern among trusted
//! workspace members, not a security hole, and lands with the M6 workbench UI,
//! which owns the session ids it creates (it will namespace them
//! `<user_id>:<client id>`). See docs/v2/m4-m5-contract.md §"Tenancy & sessions".
//!
//! ## State ownership
//! M3 owns [`crate::AppState`]; this module keeps its own state (the
//! [`lgs::TenantRegistry`], materialization locks, and per-tenant router cache)
//! in [`LgsHost`], created lazily from `&AppState` on first request via a
//! process-global map keyed by cache root. Production has exactly one cache root
//! (one config → one host); keying by root additionally isolates integration
//! tests sharing the test process.

mod dispatch;
mod materialize;

pub use dispatch::dispatch;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use object_store::ObjectStore;
use sqlx::PgPool;
use uuid::Uuid;

use lgs::{TenantId, TenantRegistry};

use crate::AppState;
use crate::config::{Config, StorageConfig};
use materialize::Materializer;

/// Identity of a revision being materialized and served, threaded through the
/// host as one argument.
pub(crate) struct RevisionRef<'a> {
    pub workspace_id: Uuid,
    pub game_id: Uuid,
    /// The DB game slug; materialized files live under `<number>/<game_slug>/`.
    pub game_slug: &'a str,
    pub number: i32,
    pub revision_id: Uuid,
}

/// Per-deployment multi-tenant LGS host: one shared [`TenantRegistry`] (with its
/// process-global decompressed-books budget), the on-disk revision
/// [`Materializer`], and a per-tenant router cache.
pub(crate) struct LgsHost {
    registry: TenantRegistry,
    materializer: Materializer,
    /// One built [`axum::Router`] per tenant. `Router` is `Clone` and implements
    /// `Service`, so a request does `clone().oneshot(req)` off the cached value.
    routers: DashMap<TenantId, axum::Router>,
    /// Per-tenant decompressed-books cap applied on registration (config now,
    /// billing later). `None` leaves tenants sharing the global budget uncapped.
    books_cap: Option<u64>,
}

/// One [`LgsHost`] per distinct cache root. In production a single `AppState`
/// yields a single cache root → a single host (the contract's one process-global
/// registry); keying by root keeps tests with separate temp storage isolated.
static HOSTS: OnceLock<DashMap<PathBuf, Arc<LgsHost>>> = OnceLock::new();

/// Resolve (creating on first use) the host for this state's cache root.
fn host_for(state: &AppState) -> Arc<LgsHost> {
    let cache_root = cache_root_for(&state.config);
    HOSTS
        .get_or_init(DashMap::new)
        .entry(cache_root.clone())
        .or_insert_with(|| {
            Arc::new(LgsHost::new(
                cache_root,
                state.config.server_math_cache_bytes,
                state.config.server_tenant_books_cap_bytes,
            ))
        })
        .clone()
}

/// The on-disk materialized cache directory for a revision, laid out exactly the
/// way [`materialize`] writes it (`<cache_root>/rev/<workspace>/<game>/<number>`).
///
/// Content-lifecycle deletion uses this to best-effort evict a just-deleted
/// revision's cache. If the directory is momentarily held open (a concurrent
/// decompression on Windows) the delete is skipped — harmless, since the LRU
/// evicts it eventually and a request for the deleted revision 404s before it can
/// ever be re-materialized.
pub fn revision_cache_dir(
    config: &Config,
    workspace_id: Uuid,
    game_id: Uuid,
    number: i32,
) -> PathBuf {
    cache_root_for(config)
        .join("rev")
        .join(workspace_id.to_string())
        .join(game_id.to_string())
        .join(number.to_string())
}

/// The materialized cache tree for a whole game — the parent of every
/// [`revision_cache_dir`] of that game. Game deletion removes it in one sweep
/// (same best-effort semantics as the per-revision variant).
pub fn game_cache_dir(config: &Config, workspace_id: Uuid, game_id: Uuid) -> PathBuf {
    cache_root_for(config)
        .join("rev")
        .join(workspace_id.to_string())
        .join(game_id.to_string())
}

/// `<STORAGE_FS_ROOT>/../cache` for the fs backend; `./data/cache` for s3 (no
/// local blob root to hang the cache off).
fn cache_root_for(config: &Config) -> PathBuf {
    match &config.storage {
        StorageConfig::Fs { root } => root
            .parent()
            .map(|parent| parent.join("cache"))
            .unwrap_or_else(|| PathBuf::from("./data/cache")),
        StorageConfig::S3 { .. } => PathBuf::from("./data/cache"),
    }
}

impl LgsHost {
    fn new(cache_root: PathBuf, cache_budget: u64, books_cap: Option<u64>) -> Self {
        Self {
            registry: TenantRegistry::new(),
            materializer: Materializer::new(cache_root, cache_budget),
            routers: DashMap::new(),
            books_cap,
        }
    }

    /// The opaque tenant id `ws:<workspace>:game:<game>:rev:<number>` — parseable
    /// here, opaque to lgs.
    fn tenant_id(workspace_id: Uuid, game_id: Uuid, number: i32) -> TenantId {
        TenantId::from(format!("ws:{workspace_id}:game:{game_id}:rev:{number}"))
    }

    /// Ensure the revision is materialized, register (idempotently) its tenant
    /// over the materialized math root, apply the per-tenant books cap, and
    /// return the tenant's router (built + cached on first use).
    async fn router_for_revision(
        &self,
        store: &dyn ObjectStore,
        pool: &PgPool,
        rev: &RevisionRef<'_>,
    ) -> anyhow::Result<axum::Router> {
        let math_root = self.materializer.ensure(store, pool, rev).await?;

        let tenant = Self::tenant_id(rev.workspace_id, rev.game_id, rev.number);
        // Atomic get-or-create: concurrent callers observe one AppState. The math
        // root is `<number>/`, so the engine resolves `<number>/<game_slug>/file`.
        self.registry.get_or_create_disk(tenant.clone(), &math_root);
        self.registry.set_tenant_cap(&tenant, self.books_cap);

        if let Some(cached) = self.routers.get(&tenant) {
            return Ok(cached.clone());
        }
        let built = self
            .registry
            .router_for(&tenant)
            .ok_or_else(|| anyhow::anyhow!("tenant {tenant} unregistered after get_or_create"))?;
        // `or_insert` collapses a lost race onto the winner's router; both are
        // equivalent (same tenant AppState), so returning either is correct.
        Ok(self.routers.entry(tenant).or_insert(built).clone())
    }
}
