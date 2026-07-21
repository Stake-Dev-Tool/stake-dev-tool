use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router, middleware};
use protocol::{ComponentStatus, HealthResponse, ServiceStatus};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::AppState;
use crate::api;
use crate::share::{self, ShareHost};
use crate::storage;

const DB_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api::router());

    // The dashboard is a static SPA (web/build). Serving it from the same
    // binary keeps cookies same-origin and makes self-hosting a single
    // artifact. Unmatched non-API paths fall back to index.html so deep links
    // like /w/:slug resolve client-side.
    match state.config.resolve_web_dir() {
        Some(dir) if dir.join("index.html").exists() => {
            tracing::info!(dir = %dir.display(), "serving dashboard");
            // `fallback` (not `not_found_service`) so the SPA shell comes back
            // as 200 — `not_found_service` would force a 404 status onto it.
            router = router.fallback_service(
                ServeDir::new(&dir).fallback(ServeFile::new(dir.join("index.html"))),
            );
        }
        Some(dir) => {
            tracing::warn!(
                dir = %dir.display(),
                "SERVER_WEB_DIR has no index.html; dashboard not served"
            );
        }
        None => {
            tracing::warn!("no dashboard build found (web/build); API-only mode");
        }
    }

    let app = router
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    // M5 — Host-based share dispatch. When a play domain is configured, requests
    // whose Host is `<label>.<play_domain>` are peeled off to the share router
    // BEFORE the app; every other Host falls through unchanged. The layer is
    // added ONLY when `play_domain` is set, so an instance without it keeps a
    // byte-identical app router (the existing health/auth tests exercise that
    // path with `play_domain: None`).
    match state.config.play_domain.clone() {
        Some(play_domain) => {
            let share_router = share::router().with_state(state);
            let dispatch = ShareDispatch {
                play_domain: Arc::from(play_domain.as_str()),
                share_router,
            };
            app.layer(middleware::from_fn_with_state(dispatch, host_dispatch))
        }
        None => app,
    }
}

/// State for the Host-dispatch middleware: the configured play domain and the
/// (already state-bound) share router.
#[derive(Clone)]
struct ShareDispatch {
    play_domain: Arc<str>,
    share_router: Router,
}

/// Peel share-host requests off to the share router; forward everything else to
/// the app. Share hosts never reach the app (its cookies live on a different
/// registrable domain), and app hosts never touch the share router.
async fn host_dispatch(
    State(dispatch): State<ShareDispatch>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(label) = share::match_share_label(req.headers(), &dispatch.play_domain) {
        req.extensions_mut().insert(ShareHost(label));
        return match dispatch.share_router.clone().oneshot(req).await {
            Ok(response) => response,
            Err(infallible) => match infallible {},
        };
    }
    next.run(req).await
}

/// Liveness + readiness in one probe: 200 when every dependency answers, 503
/// (`degraded`) when any is down. The body always describes each component so a
/// 503 is diagnosable without shelling into the box.
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let db = check_db(&state).await;
    let object_store = check_object_store(&state).await;
    let healthy = db.ok && object_store.ok;

    let body = HealthResponse {
        status: if healthy {
            ServiceStatus::Ok
        } else {
            ServiceStatus::Degraded
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
        db,
        object_store,
    };
    let code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

async fn check_db(state: &AppState) -> ComponentStatus {
    let query = sqlx::query("SELECT 1").execute(&state.pool);
    match tokio::time::timeout(DB_CHECK_TIMEOUT, query).await {
        Ok(Ok(_)) => ComponentStatus {
            ok: true,
            error: None,
        },
        Ok(Err(e)) => ComponentStatus {
            ok: false,
            error: Some(e.to_string()),
        },
        Err(_) => ComponentStatus {
            ok: false,
            error: Some("database health check timed out".to_string()),
        },
    }
}

async fn check_object_store(state: &AppState) -> ComponentStatus {
    match storage::health_probe(state.store.as_ref()).await {
        Ok(()) => ComponentStatus {
            ok: true,
            error: None,
        },
        Err(e) => ComponentStatus {
            ok: false,
            error: Some(e.to_string()),
        },
    }
}
