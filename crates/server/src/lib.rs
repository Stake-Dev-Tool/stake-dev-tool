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
pub mod mail;
pub mod share;
pub mod stats;
pub mod stats_analysis;
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
    /// Process-local limiter for the email-sending endpoints (forgot-password /
    /// resend-verification): 5 per hour per `(ip, email)`.
    pub email_limiter: Arc<LoginRateLimiter>,
    /// Reused HTTP client for GitHub/Discord OAuth calls and Resend email.
    pub http_client: reqwest::Client,
    /// Per-workspace realtime fan-out behind the SSE stream (M3).
    pub events: Arc<crate::documents::WorkspaceEvents>,
    /// 60s cache of custom-domain -> workspace resolutions, shared by the
    /// Host-dispatch layer and the `/api/tls-check` ask endpoint. Cleared on any
    /// custom-domain write so a set/clear takes effect immediately.
    pub custom_domains: Arc<crate::share::custom::CustomDomainCache>,
}

impl AppState {
    pub fn new(config: Config, pool: PgPool, store: Arc<dyn ObjectStore>) -> Self {
        Self {
            config: Arc::new(config),
            pool,
            store,
            login_limiter: Arc::new(LoginRateLimiter::new()),
            email_limiter: Arc::new(LoginRateLimiter::with_limits(
                5,
                std::time::Duration::from_secs(3600),
            )),
            http_client: reqwest::Client::new(),
            events: Arc::new(crate::documents::WorkspaceEvents::new()),
            custom_domains: Arc::new(crate::share::custom::CustomDomainCache::new()),
        }
    }
}
