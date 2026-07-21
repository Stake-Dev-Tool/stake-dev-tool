//! Request dispatch for `/api/ws/:slug/g/:game/r/:number/*rest`.
//!
//! Resolves the workspace + membership (the auth boundary for the otherwise-
//! unauthenticated inner LGS), resolves the game + revision, ensures the
//! revision is materialized, then forwards the request into the tenant's router
//! with the prefix stripped so the LGS sees its normal absolute paths.

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

use super::{RevisionRef, host_for};

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

    // --- MATERIALIZE + RESOLVE TENANT ROUTER ---------------------------------
    let host = host_for(&state);
    let rev = RevisionRef {
        workspace_id: workspace.id,
        game_id,
        game_slug: &game,
        number,
        revision_id,
    };
    let router = host
        .router_for_revision(state.store.as_ref(), &state.pool, &rev)
        .await
        .map_err(ApiError::internal)?;

    // --- URI REWRITE ----------------------------------------------------------
    // Strip the `/ws/:slug/g/:game/r/:number` prefix: the inner LGS must see the
    // exact absolute path it serves standalone (`/api/rgs/…`, `/api/devtool/…`,
    // `/bet/replay/…`). `rest` carries no leading slash; the query string,
    // method, version, headers, and body are preserved verbatim.
    //
    // The request is rebuilt from method/headers/body rather than reusing the
    // original `Parts` on purpose: that DROPS the outer router's matched
    // path-param extension. axum accumulates path params across routers, so a
    // forwarded request would otherwise make the inner LGS `Path` extractor see
    // this route's 4 params *plus* its own (e.g. "expected 1 but got 5"). The
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
