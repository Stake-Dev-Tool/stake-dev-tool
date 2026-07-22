//! The LGS router's CORS layer is a security boundary (2026-07-22 audit): the
//! DESKTOP mount keeps the permissive mirror-origin-with-credentials layer the
//! local game front-end needs, while the CLOUD multi-tenant mounts (workbench
//! `/ws/…` and public share hosts) — which are same-origin with the LGS — must
//! carry no permissive CORS at all.
//!
//! Driven entirely through `lgs`'s public router API, so it needs no database and
//! always runs. `lgs::build_router` is the desktop/standalone entry point;
//! `TenantRegistry::router_for` is what `crates/server` mounts per tenant.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use lgs::{TenantId, TenantRegistry};
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:5173";

/// A CORS preflight for one of the LGS wallet routes.
fn preflight() -> Request<Body> {
    Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/rgs/demo/wallet/authenticate")
        .header("origin", ORIGIN)
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap()
}

async fn allow_origin_header(router: axum::Router) -> Option<String> {
    let resp = router.oneshot(preflight()).await.unwrap();
    resp.headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[tokio::test]
async fn desktop_router_keeps_permissive_cors() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = TenantRegistry::new();
    let state = registry.get_or_create_disk(TenantId::from("t"), tmp.path());

    // The desktop / standalone entry point mirrors the request origin and allows
    // credentials — a real preflight comes back approved.
    let resp = lgs::build_router(state, None)
        .oneshot(preflight())
        .await
        .unwrap();
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some(ORIGIN),
        "desktop CORS must mirror the request origin"
    );
    assert_eq!(
        resp.headers()
            .get("access-control-allow-credentials")
            .and_then(|v| v.to_str().ok()),
        Some("true"),
        "desktop CORS must allow credentials"
    );
}

#[tokio::test]
async fn cloud_tenant_router_has_no_permissive_cors() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = TenantRegistry::new();
    let tenant = TenantId::from("t");
    registry.get_or_create_disk(tenant.clone(), tmp.path());

    // The per-tenant router the cloud mounts must NOT emit any CORS allowance.
    let router = registry.router_for(&tenant).expect("tenant registered");
    assert_eq!(
        allow_origin_header(router).await,
        None,
        "cloud tenant mount must not mirror arbitrary origins"
    );

    // Explicitly building with SameOrigin is equivalent.
    let state = registry.get_or_create_disk(tenant.clone(), tmp.path());
    let router = lgs::build_router_with_cors(state, None, lgs::CorsMode::SameOrigin);
    assert_eq!(allow_origin_header(router).await, None);
}

#[tokio::test]
async fn same_origin_router_still_serves_the_routes() {
    // No CORS does not mean no routing: a same-origin preflight simply isn't
    // special-cased, so OPTIONS on a POST-only route is a normal 405 (not a CORS
    // 200), and the route itself is still present.
    let tmp = tempfile::tempdir().unwrap();
    let registry = TenantRegistry::new();
    let tenant = TenantId::from("t");
    registry.get_or_create_disk(tenant.clone(), tmp.path());
    let router = registry.router_for(&tenant).unwrap();

    let resp = router.oneshot(preflight()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}
