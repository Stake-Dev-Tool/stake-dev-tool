pub mod config;
pub mod devtool;
pub mod error;
pub mod math_engine;
pub mod replay;
pub mod routes;
pub mod saved_rounds;
pub mod session;
pub mod settings;
pub mod state;
pub mod tenant;
pub mod tls;
pub mod types;

// The multi-tenant surface `crates/server` builds on. Single-tenant callers
// (standalone binary, desktop app) never need these — they keep using
// `MathEngine::new` / `AppState::new`, which run under `TenantId::default`.
pub use math_engine::{BooksCache, DiskMathSource, MathSource};
pub use tenant::{TenantId, TenantRegistry};

use crate::config::ServerConfig;
use crate::math_engine::MathEngine;
use crate::state::AppState;
use axum::http::{HeaderName, Method};
use axum_server::tls_rustls::RustlsConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Once;
use std::time::Duration;
use tokio::sync::oneshot;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

pub struct ServerHandle {
    pub bound_addr: SocketAddr,
    pub shutdown: oneshot::Sender<()>,
    pub join: tokio::task::JoinHandle<std::io::Result<()>>,
}

/// UI bundle embedded at compile time for release builds. In debug builds the
/// UI is served from disk (see `ui_dir` fallback below) so `vite build` +
/// `cargo run` picks up fresh UI without rebuilding the Rust binary.
#[cfg(not(debug_assertions))]
static EMBEDDED_UI: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../ui/build");

pub fn build_router(state: Arc<AppState>, ui_dir: Option<std::path::PathBuf>) -> axum::Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("authorization"),
            HeaderName::from_static("x-requested-with"),
        ])
        .allow_credentials(true)
        .allow_private_network(true)
        .expose_headers([HeaderName::from_static("content-type")]);

    let mut router = routes::router(state.clone())
        .merge(devtool::router(state.clone()))
        .merge(replay::router(state));

    // In release builds, always serve the embedded UI (no filesystem dep).
    // In debug builds, fall back to the on-disk ui/build/ for hot iteration.
    #[cfg(not(debug_assertions))]
    {
        let _ = ui_dir; // unused in release
        router = router.fallback(embedded_ui_handler);
    }
    #[cfg(debug_assertions)]
    if let Some(dir) = ui_dir {
        use tower_http::services::{ServeDir, ServeFile};
        let fallback = ServeFile::new(dir.join("index.html"));
        router = router.fallback_service(ServeDir::new(dir).fallback(fallback));
    }

    router.layer(cors).layer(TraceLayer::new_for_http())
}

#[cfg(not(debug_assertions))]
async fn embedded_ui_handler(
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    let raw_path = req.uri().path();
    let trimmed = raw_path.trim_start_matches('/');
    // Try direct match, directory index, then SPA fallback (index.html).
    let candidates: [&str; 3] = [trimmed, &format!("{trimmed}/index.html"), "index.html"];
    // Re-alloc to satisfy lifetime — String → &str.
    let owned: Vec<String> = candidates.iter().map(|s| s.to_string()).collect();
    for name in owned {
        if let Some(file) = EMBEDDED_UI.get_file(&name) {
            let mime = mime_guess::from_path(name)
                .first_or_octet_stream()
                .to_string();
            return ([(header::CONTENT_TYPE, mime)], Body::from(file.contents())).into_response();
        }
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

static CRYPTO_INIT: Once = Once::new();

fn init_crypto_provider() {
    CRYPTO_INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

async fn make_local_ca_tls_config() -> anyhow::Result<RustlsConfig> {
    let ca = tls::LocalCa::load_or_create().await?;
    let leaf = ca.leaf_bundle();
    let config =
        RustlsConfig::from_pem(leaf.cert_pem.into_bytes(), leaf.key_pem.into_bytes()).await?;
    Ok(config)
}

pub async fn start_server(cfg: ServerConfig) -> anyhow::Result<ServerHandle> {
    let bind = cfg.bind_addr.clone();
    let ui_dir = cfg.ui_dir.clone();
    let engine = MathEngine::new(cfg);
    let app_state = Arc::new(AppState::new(engine));
    start_server_with_state(app_state, bind, ui_dir).await
}

pub async fn start_server_with_state(
    app_state: Arc<AppState>,
    bind: String,
    ui_dir: Option<std::path::PathBuf>,
) -> anyhow::Result<ServerHandle> {
    init_crypto_provider();

    let app = build_router(app_state, ui_dir);

    let tls_config = make_local_ca_tls_config().await?;

    let std_listener = std::net::TcpListener::bind(&bind)?;
    let bound_addr = std_listener.local_addr()?;

    let handle = axum_server::Handle::new();
    let handle_for_shutdown = handle.clone();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        handle_for_shutdown.graceful_shutdown(Some(Duration::from_secs(5)));
    });

    let join = tokio::spawn(async move {
        axum_server::from_tcp_rustls(std_listener, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
    });

    Ok(ServerHandle {
        bound_addr,
        shutdown: shutdown_tx,
        join,
    })
}
