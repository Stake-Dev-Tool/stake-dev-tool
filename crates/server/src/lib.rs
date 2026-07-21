pub mod api;
pub mod auth;
pub mod billing;
pub mod blobs;
pub mod config;
pub mod db;
pub mod documents;
pub mod error;
pub mod http;
pub mod lgs_host;
pub mod share;
pub mod stats;
pub mod storage;

use std::sync::Arc;

use object_store::ObjectStore;
use sqlx::PgPool;

use crate::auth::ratelimit::LoginRateLimiter;
use crate::config::Config;

/// Shared, cheaply-clonable application state. `PgPool`, the `Arc` fields, and
/// `reqwest::Client` are all reference-counted, so cloning is just a bump of a
/// few counters — which is what axum needs to hand a copy to every request
/// handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
    pub store: Arc<dyn ObjectStore>,
    /// Process-local failed-login limiter (see `auth::ratelimit`).
    pub login_limiter: Arc<LoginRateLimiter>,
    /// Reused HTTP client for GitHub OAuth calls.
    pub http_client: reqwest::Client,
    /// Per-workspace realtime fan-out behind the SSE stream (M3).
    pub events: Arc<crate::documents::WorkspaceEvents>,
}

impl AppState {
    pub fn new(config: Config, pool: PgPool, store: Arc<dyn ObjectStore>) -> Self {
        Self {
            config: Arc::new(config),
            pool,
            store,
            login_limiter: Arc::new(LoginRateLimiter::new()),
            http_client: reqwest::Client::new(),
            events: Arc::new(crate::documents::WorkspaceEvents::new()),
        }
    }
}
