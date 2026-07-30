//! Request dispatch for the two tenant LGS mounts.
//!
//! Both resolve an authorization, then hand off to the same [`forward`]: ensure
//! the revision is materialized and forward the request into the tenant's router
//! with the prefix stripped so the LGS sees its normal absolute paths. They only
//! differ in how the caller proves who they are:
//!
//! - [`dispatch`] — `/api/ws/:slug/g/:game/r/:number/*rest`, authenticated by the
//!   session cookie (or a PAT). The same-origin workbench path.
//! - [`dispatch_workbench`] — `/api/wb/:token/*rest`, authenticated by a
//!   capability token in the path ([`super::workbench`]). The cross-origin path,
//!   for a game front served from a developer's own dev server, where a
//!   `SameSite=Lax` cookie would never be sent.
//!
//! Membership is re-checked on every request in both, so losing workspace access
//! closes both doors immediately.

use axum::extract::{Path, Request, State};
use axum::http::Uri;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::AppState;
use crate::api::workspaces::{require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::error::{ApiError, ApiResult};

use super::{RevisionRef, host_for, workbench};

/// All-methods handler for the tenant-scoped LGS mount. `rest` is the wildcard
/// tail (e.g. `api/devtool/games/demo/modes`, `api/rgs/demo/wallet/play`), which
/// already starts with the inner LGS's own `api/…` / `bet/…` prefix.
pub async fn dispatch(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game, number, rest)): Path<(String, String, i32, String)>,
    req: Request,
) -> ApiResult<Response> {
    // --- AUTH BOUNDARY --------------------------------------------------------
    // The inner LGS routes are unauthenticated by design; membership IS the
    // gate. A non-member 404s (never learning the workspace/game/revision
    // exists) before any tenant machinery is touched.
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;
    let (game_id, revision_id) =
        resolve_game_and_revision(&state.pool, workspace.id, &game, number).await?;

    let rev = RevisionRef {
        workspace_id: workspace.id,
        game_id,
        game_slug: &game,
        number,
        revision_id,
    };
    forward(&state, rev, &rest, req).await
}

/// All-methods handler for the CROSS-ORIGIN mount, `/api/wb/:token/*rest`. The
/// path token is the only credential — no cookie, no header — so this is
/// reachable from a game front on any origin (see [`super::workbench`]).
///
/// The auth boundary is identical in strength to [`dispatch`]: the token names a
/// user and a pinned `(workspace, game, revision)`, and membership is re-checked
/// here, so a token can never outlive the access it was minted under.
pub async fn dispatch_workbench(
    State(state): State<AppState>,
    Path((token, rest)): Path<(String, String)>,
    req: Request,
) -> ApiResult<Response> {
    let grant = workbench::resolve(&state.pool, &token).await?;
    require_membership(&state.pool, grant.workspace_id, grant.user_id).await?;

    let rev = RevisionRef {
        workspace_id: grant.workspace_id,
        game_id: grant.game_id,
        game_slug: &grant.game_slug,
        number: grant.revision_number,
        revision_id: grant.revision_id,
    };
    forward(&state, rev, &rest, req).await
}

/// Materialize the revision, resolve its tenant router, and forward the request
/// with the mount prefix stripped. Shared by both mounts — everything above this
/// point is authorization, everything below is plumbing.
async fn forward(
    state: &AppState,
    rev: RevisionRef<'_>,
    rest: &str,
    req: Request,
) -> ApiResult<Response> {
    // --- MATERIALIZE + RESOLVE TENANT ROUTER ---------------------------------
    let host = host_for(state);
    let router = host
        .router_for_revision(state.store.as_ref(), &state.pool, &rev)
        .await
        .map_err(ApiError::internal)?;

    // --- URI REWRITE ----------------------------------------------------------
    // Strip the mount prefix (`/ws/:slug/g/:game/r/:number` or `/wb/:token`): the
    // inner LGS must see the exact absolute path it serves standalone
    // (`/api/rgs/…`, `/api/devtool/…`,
    // `/bet/replay/…`). `rest` carries no leading slash; the query string,
    // method, version, headers, and body are preserved verbatim.
    //
    // The request is rebuilt from method/headers/body rather than reusing the
    // original `Parts` on purpose: that DROPS the outer router's matched
    // path-param extension. axum accumulates path params across routers, so a
    // forwarded request would otherwise make the inner LGS `Path` extractor see
    // the mount route's params *plus* its own (e.g. "expected 1 but got 5"). The
    // inner LGS extracts no other request extensions, so dropping them is safe.
    let rest = rest.trim_start_matches('/');
    let path_and_query = match req.uri().query() {
        Some(query) => format!("/{rest}?{query}"),
        None => format!("/{rest}"),
    };
    let uri = Uri::try_from(&path_and_query).map_err(|e| {
        ApiError::bad_request(
            "bad_forward_path",
            format!("cannot rewrite request path: {e}"),
        )
    })?;
    let (parts, body) = req.into_parts();
    let mut inner = Request::builder()
        .method(parts.method)
        .uri(uri)
        .version(parts.version)
        .body(body)
        .map_err(ApiError::internal)?;
    *inner.headers_mut() = parts.headers;

    // `Router` is an infallible `Service`; forward once and relay the response.
    Ok(router.oneshot(inner).await.into_response())
}

/// Resolve `(game_id, revision_id)` from a workspace-scoped game slug + revision
/// number, 404ing each miss independently.
async fn resolve_game_and_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    game_slug: &str,
    number: i32,
) -> ApiResult<(Uuid, Uuid)> {
    let game_id: Uuid =
        sqlx::query_scalar("SELECT id FROM games WHERE workspace_id = $1 AND slug = $2")
            .bind(workspace_id)
            .bind(game_slug)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::not_found("game_not_found", "no such game"))?;

    let revision_id: Uuid =
        sqlx::query_scalar("SELECT id FROM revisions WHERE game_id = $1 AND number = $2")
            .bind(game_id)
            .bind(number)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::not_found("revision_not_found", "no such revision"))?;

    Ok((game_id, revision_id))
}
