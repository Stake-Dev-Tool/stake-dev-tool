pub mod config;
pub mod db;
pub mod http;
pub mod storage;

use std::sync::Arc;

use object_store::ObjectStore;
use sqlx::PgPool;

use crate::config::Config;

/// Shared, cheaply-clonable application state. `PgPool` and the `Arc` fields are
/// all reference-counted, so cloning is just a bump of a few counters — which is
/// what axum needs to hand a copy to every request handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
    pub store: Arc<dyn ObjectStore>,
}

impl AppState {
    pub fn new(config: Config, pool: PgPool, store: Arc<dyn ObjectStore>) -> Self {
        Self {
            config: Arc::new(config),
            pool,
            store,
        }
    }
}
