//! Axum extractors that turn a request's credentials into a `CurrentUser`.
//!
//! `CurrentUser` accepts either a session cookie or an `Authorization: Bearer
//! sdt_pat_…` token. `SessionUser` additionally requires the cookie, so a PAT
//! can't reach session-only endpoints (e.g. minting more tokens). `ClientIp`
//! resolves a best-effort client address for login rate-limiting.

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::async_trait;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum_extra::TypedHeader;
use axum_extra::extract::cookie::CookieJar;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Bearer;
use uuid::Uuid;

use crate::AppState;
use crate::auth::{sessions, tokens};
use crate::error::{ApiError, ApiResult};

/// Which credential authenticated the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    Session,
    ApiToken,
}

/// An authenticated caller: the user, how they authenticated, and their scopes
/// (a session implicitly has full scopes).
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user_id: Uuid,
    pub source: AuthSource,
    pub scopes: Vec<String>,
}

/// Core resolution shared by both extractors. A present-but-invalid Bearer token
/// is rejected outright rather than falling through to the cookie.
async fn authenticate(parts: &mut Parts, state: &AppState) -> ApiResult<CurrentUser> {
    if let Ok(Some(TypedHeader(auth))) =
        Option::<TypedHeader<Authorization<Bearer>>>::from_request_parts(parts, state).await
    {
        if let Some(token) = tokens::lookup_token(&state.pool, auth.token()).await? {
            return Ok(CurrentUser {
                user_id: token.user_id,
                source: AuthSource::ApiToken,
                scopes: token.scopes,
            });
        }
        return Err(ApiError::unauthorized(
            "invalid_token",
            "the bearer token is invalid, expired, or revoked",
        ));
    }

    let jar = CookieJar::from_headers(&parts.headers);
    if let Some(cookie) = jar.get(sessions::SESSION_COOKIE)
        && let Some(user_id) = sessions::lookup_session(&state.pool, cookie.value()).await?
    {
        return Ok(CurrentUser {
            user_id,
            source: AuthSource::Session,
            scopes: vec!["full".to_string()],
        });
    }

    Err(ApiError::unauthorized(
        "unauthenticated",
        "authentication required",
    ))
}

#[async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(parts, state).await
    }
}

/// A caller authenticated specifically by a browser session. Reusing
/// `CurrentUser`'s logic, it rejects API-token callers with 403.
pub struct SessionUser(pub CurrentUser);

#[async_trait]
impl FromRequestParts<AppState> for SessionUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = authenticate(parts, state).await?;
        if user.source != AuthSource::Session {
            return Err(ApiError::forbidden(
                "session_required",
                "this action requires a browser session, not an API token",
            ));
        }
        Ok(SessionUser(user))
    }
}

/// Best-effort client IP for rate-limit keying.
pub struct ClientIp(pub String);

#[async_trait]
impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(resolve_ip(parts)))
    }
}

/// Trusts `X-Forwarded-For` / `X-Real-IP` (set by our reverse proxy) ahead of
/// the socket address, falling back to a constant so keying still works in
/// tests and direct-connection setups.
fn resolve_ip(parts: &Parts) -> String {
    if let Some(xff) = parts
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        && let Some(first) = xff.split(',').next()
    {
        let ip = first.trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }
    if let Some(xri) = parts.headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let ip = xri.trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }
    if let Some(ConnectInfo(addr)) = parts.extensions.get::<ConnectInfo<SocketAddr>>() {
        return addr.ip().to_string();
    }
    "unknown".to_string()
}
