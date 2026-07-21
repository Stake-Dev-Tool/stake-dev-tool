//! Multi-tenant plumbing for embedding the LGS engine in a server that hosts
//! many isolated `(workspace, game, revision)` math roots in a single process.
//!
//! The standalone binary and the desktop app are *single-tenant*: they run
//! under the well-known [`TenantId::default`] and never touch the
//! [`TenantRegistry`]. Their code paths are byte-identical to before this
//! module existed — `MathEngine::new` / `AppState::new` still build exactly the
//! same thing, they just happen to be labelled with the default tenant now.
//!
//! The future `crates/server` drives multi-tenancy through [`TenantRegistry`]:
//! it registers one tenant per `(workspace, game, revision)` (encoded into the
//! opaque [`TenantId`]), each backed by its own [`MathSource`] and
//! [`SessionStore`], and mounts a per-tenant router via
//! [`TenantRegistry::router_for`]. Every tenant shares one process-global
//! [`BooksCache`], so the decompressed-books memory budget is enforced once for
//! the whole process while an optional per-tenant cap keeps one tenant from
//! evicting everyone else.

use crate::math_engine::{BooksCache, DiskMathSource, MathEngine, MathSource};
use crate::session::SessionStore;
use crate::state::AppState;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Opaque tenant identity.
///
/// Deliberately a small owned string so the server can encode
/// `(workspace, game, revision)` into it later (M2/M4) without another
/// refactor — nothing in this crate interprets its contents. Backed by an
/// `Arc<str>` so cloning is O(1) (an atomic ref-count bump), which keeps it
/// cheap to carry on the hot path and to use as a map key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TenantId(Arc<str>);

impl TenantId {
    /// The single-tenant identity used by the standalone binary and the
    /// desktop app. All pre-multi-tenant behavior runs under this tenant.
    pub const DEFAULT_STR: &'static str = "default";

    /// Build a tenant id from anything convertible into an `Arc<str>`.
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    /// Borrow the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self(Arc::from(Self::DEFAULT_STR))
    }
}

impl std::fmt::Display for TenantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for TenantId {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<String> for TenantId {
    fn from(s: String) -> Self {
        Self(Arc::from(s.as_str()))
    }
}

/// Registry of live tenants for the multi-tenant server.
///
/// Each tenant maps to its own [`AppState`] — an isolated [`SessionStore`],
/// force-event slot, game-config cache, and [`MathSource`] — while all tenants
/// share one [`BooksCache`] so the decompressed-books budget is process-global.
/// Lookups ([`TenantRegistry::get`]) are O(1) and contention-free: a request is
/// routed to its tenant once, then operates on the returned `Arc<AppState>`
/// directly.
///
/// The desktop and standalone servers do not use this type at all; it exists
/// purely as the programmatic surface `crates/server` will build on.
pub struct TenantRegistry {
    tenants: DashMap<TenantId, Arc<AppState>>,
    books: Arc<BooksCache>,
}

impl TenantRegistry {
    /// A registry whose shared books cache uses the default process-global
    /// budget.
    pub fn new() -> Self {
        Self::with_books_cache(Arc::new(BooksCache::new()))
    }

    /// A registry over a caller-provided books cache. Lets the server size the
    /// global budget or share one cache across several registries.
    pub fn with_books_cache(books: Arc<BooksCache>) -> Self {
        Self {
            tenants: DashMap::new(),
            books,
        }
    }

    /// The shared books cache. Useful for setting per-tenant caps or inspecting
    /// the global budget directly.
    pub fn books_cache(&self) -> &Arc<BooksCache> {
        &self.books
    }

    /// O(1), contention-free lookup of an already-registered tenant.
    pub fn get(&self, tenant: &TenantId) -> Option<Arc<AppState>> {
        self.tenants.get(tenant).map(|s| Arc::clone(s.value()))
    }

    /// Register a tenant (replacing any existing registration), backed by
    /// `source` for math and `sessions` for wallet state. Returns the tenant's
    /// [`AppState`], which the server mounts as the axum state for that
    /// tenant's routes. The engine is wired to the registry's shared
    /// [`BooksCache`].
    pub fn insert(
        &self,
        tenant: TenantId,
        source: Arc<dyn MathSource>,
        sessions: Arc<SessionStore>,
    ) -> Arc<AppState> {
        let engine = Arc::new(MathEngine::with_source(
            tenant.clone(),
            source,
            Arc::clone(&self.books),
        ));
        let state = Arc::new(AppState::from_parts(sessions, engine));
        self.tenants.insert(tenant, Arc::clone(&state));
        state
    }

    /// Get-or-create a tenant whose math lives under `math_root` on disk, with
    /// an isolated in-memory session store. This is the ergonomic entry point
    /// for the server, which materializes a revision's math into a per-tenant
    /// local dir and wants ephemeral, fully isolated sessions.
    ///
    /// The get-or-create is atomic: concurrent callers for the same tenant all
    /// observe the same `AppState`.
    pub fn get_or_create_disk(
        &self,
        tenant: TenantId,
        math_root: impl Into<PathBuf>,
    ) -> Arc<AppState> {
        // Fast path: already registered.
        if let Some(existing) = self.get(&tenant) {
            return existing;
        }
        let source: Arc<dyn MathSource> = Arc::new(DiskMathSource::new(math_root));
        let books = Arc::clone(&self.books);
        let tenant_for_engine = tenant.clone();
        // `or_insert_with` runs only if the entry is still absent, so a lost
        // race drops the freshly-built (and unused) state instead of clobbering
        // the winner. `.clone()` derefs the map guard to clone the `Arc`.
        self.tenants
            .entry(tenant)
            .or_insert_with(|| {
                let engine = Arc::new(MathEngine::with_source(tenant_for_engine, source, books));
                Arc::new(AppState::from_parts(
                    Arc::new(SessionStore::in_memory()),
                    engine,
                ))
            })
            .clone()
    }

    /// Remove a tenant, returning its [`AppState`] if it was registered.
    ///
    /// In-flight requests holding an `Arc<AppState>` keep running safely; this
    /// only stops new lookups from resolving the tenant. Any books this
    /// tenant left in the shared cache age out through the normal LRU.
    pub fn remove(&self, tenant: &TenantId) -> Option<Arc<AppState>> {
        self.tenants.remove(tenant).map(|(_, state)| state)
    }

    /// Set (or clear, with `None`) the maximum decompressed bytes this tenant
    /// may hold in the shared books cache. Default is uncapped.
    pub fn set_tenant_cap(&self, tenant: &TenantId, max_bytes: Option<u64>) {
        self.books.set_tenant_cap(tenant, max_bytes);
    }

    /// Build an axum router scoped to one registered tenant — the exact same
    /// routes and behavior as the single-tenant standalone server, bound to
    /// that tenant's state. Returns `None` if the tenant is not registered.
    ///
    /// Host/path → tenant dispatch (e.g. `*.play.<domain>`) lives above this in
    /// the server and is intentionally out of scope here.
    pub fn router_for(&self, tenant: &TenantId) -> Option<axum::Router> {
        let state = self.get(tenant)?;
        Some(
            crate::routes::router(Arc::clone(&state))
                .merge(crate::devtool::router(Arc::clone(&state)))
                .merge(crate::replay::router(state)),
        )
    }
}

impl Default for TenantRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionInit;
    use crate::state::ForcedEvent;

    fn write_index(dir: &std::path::Path, game: &str, index_json: &str) {
        let game_dir = dir.join(game);
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        std::fs::write(game_dir.join("index.json"), index_json).expect("write index.json");
    }

    #[tokio::test]
    async fn same_slug_two_tenants_get_distinct_math_sessions_and_forces() {
        let tmp_a = tempfile::tempdir().expect("tmp a");
        let tmp_b = tempfile::tempdir().expect("tmp b");
        // Same game slug "demo" in both tenants, but different math.
        write_index(
            tmp_a.path(),
            "demo",
            r#"{"modes":[{"name":"base","cost":1,"events":"b.zst","weights":"w.csv"}]}"#,
        );
        write_index(
            tmp_b.path(),
            "demo",
            r#"{"modes":[
                {"name":"base","cost":1,"events":"b.zst","weights":"w.csv"},
                {"name":"bonus","cost":100,"events":"b2.zst","weights":"w2.csv"}
            ]}"#,
        );

        let registry = TenantRegistry::new();
        let a = TenantId::from("workspace-a");
        let b = TenantId::from("workspace-b");
        let state_a = registry.get_or_create_disk(a.clone(), tmp_a.path());
        let state_b = registry.get_or_create_disk(b.clone(), tmp_b.path());

        // Distinct math: the same slug resolves to each tenant's own config.
        let cfg_a = state_a.engine.load_config("demo").await.expect("cfg a");
        let cfg_b = state_b.engine.load_config("demo").await.expect("cfg b");
        assert_eq!(cfg_a.modes.len(), 1);
        assert_eq!(cfg_b.modes.len(), 2);

        // Distinct sessions: same session id, fully isolated stores.
        state_a.sessions.upsert(
            "s1",
            SessionInit {
                game: "demo".to_string(),
                language: None,
                balance: Some(500),
                currency: None,
            },
        );
        assert!(state_b.sessions.get("s1").is_none());
        assert_eq!(state_a.sessions.get("s1").expect("session a").balance, 500);

        // Distinct force-event state.
        *state_a.forced_event.lock() = Some(ForcedEvent {
            mode: "base".to_string(),
            event_id: 7,
        });
        assert!(state_b.forced_event.lock().is_none());

        // The engines share one books cache, but under distinct tenants.
        assert_eq!(state_a.engine.tenant(), &a);
        assert_eq!(state_b.engine.tenant(), &b);
        assert!(registry.router_for(&a).is_some());
        assert!(registry.router_for(&TenantId::from("nobody")).is_none());
    }

    #[tokio::test]
    async fn get_or_create_is_idempotent_per_tenant() {
        let tmp = tempfile::tempdir().expect("tmp");
        let registry = TenantRegistry::new();
        let t = TenantId::from("ws");
        let s1 = registry.get_or_create_disk(t.clone(), tmp.path());
        let s2 = registry.get_or_create_disk(t.clone(), tmp.path());
        assert!(Arc::ptr_eq(&s1, &s2));
        assert!(Arc::ptr_eq(&registry.get(&t).expect("registered"), &s1));
    }

    #[test]
    fn insert_and_remove_tenant() {
        let tmp = tempfile::tempdir().expect("tmp");
        let registry = TenantRegistry::new();
        let t = TenantId::from("ws");
        let state = registry.insert(
            t.clone(),
            Arc::new(DiskMathSource::new(tmp.path())),
            Arc::new(SessionStore::in_memory()),
        );
        assert!(Arc::ptr_eq(&registry.get(&t).expect("registered"), &state));
        registry.set_tenant_cap(&t, Some(1024));
        assert!(registry.remove(&t).is_some());
        assert!(registry.get(&t).is_none());
    }

    #[test]
    fn default_tenant_id_is_stable() {
        assert_eq!(TenantId::default().as_str(), TenantId::DEFAULT_STR);
        assert_eq!(TenantId::new("x").as_str(), "x");
        assert_eq!(TenantId::from(String::from("y")).as_str(), "y");
    }
}
