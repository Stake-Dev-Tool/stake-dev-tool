//! Handlers for `/api/auth/*`: registration, password login/logout, the current
//! user, provider flags, GitHub OAuth, and the device pairing flow.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{DateTime, Utc};
use protocol::{
    DeviceApproveRequest, DeviceApproveResponse, DeviceCodeResponse, DeviceTokenRequest,
    LoginRequest, ProvidersResponse, RegisterRequest, User, UserResponse,
};
use serde::Deserialize;
use sqlx::PgPool;
use time::Duration as CookieDuration;
use uuid::Uuid;

use crate::AppState;
use crate::auth::device::{self, DevicePoll};
use crate::auth::extract::{ClientIp, CurrentUser, SessionUser};
use crate::auth::{generate_secret, github, passwords, sessions};
use crate::error::{ApiError, ApiResult, is_unique_violation};

const MIN_PASSWORD_LEN: usize = 8;

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: r.id,
            email: r.email,
            display_name: r.display_name,
            created_at: r.created_at,
        }
    }
}

/// Registers a password account and starts a session.
pub async fn register(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<(CookieJar, Json<UserResponse>)> {
    let email = req.email.trim();
    validate_email(email)?;
    if req.password.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::unprocessable(
            "weak_password",
            format!("password must be at least {MIN_PASSWORD_LEN} characters"),
        ));
    }
    let display_name = req.display_name.trim();
    if display_name.is_empty() {
        return Err(ApiError::unprocessable(
            "invalid_display_name",
            "display_name must not be empty",
        ));
    }

    let password_hash = passwords::hash_password(&req.password)?;
    let result = sqlx::query_as::<_, UserRow>(
        "INSERT INTO users (email, password_hash, display_name) VALUES ($1, $2, $3) \
         RETURNING id, email, display_name, created_at",
    )
    .bind(email)
    .bind(&password_hash)
    .bind(display_name)
    .fetch_one(&state.pool)
    .await;

    let user = match result {
        Ok(user) => user,
        Err(e) if is_unique_violation(&e) => {
            return Err(ApiError::conflict(
                "email_taken",
                "an account with that email already exists",
            ));
        }
        Err(e) => return Err(e.into()),
    };

    let secret = sessions::create_session(&state.pool, user.id).await?;
    let jar = jar.add(sessions::session_cookie(secret, state.config.cookie_secure));
    Ok((jar, Json(UserResponse { user: user.into() })))
}

#[derive(sqlx::FromRow)]
struct AuthRow {
    id: Uuid,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    password_hash: Option<String>,
}

/// Password login. Returns one uniform 401 whether the email is unknown or the
/// password is wrong, and is rate-limited per (IP, email).
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<UserResponse>)> {
    let email = req.email.trim();

    if state.login_limiter.is_blocked(&ip, email) {
        return Err(ApiError::too_many_requests(
            "rate_limited",
            "too many failed login attempts; try again later",
        ));
    }

    let invalid = || ApiError::unauthorized("invalid_credentials", "invalid email or password");

    let row = sqlx::query_as::<_, AuthRow>(
        "SELECT id, email, display_name, created_at, password_hash FROM users \
         WHERE lower(email) = lower($1)",
    )
    .bind(email)
    .fetch_optional(&state.pool)
    .await?;

    // A missing account, a GitHub-only account (NULL hash), and a wrong password
    // all take the same branch — and all burn the same argon2 work (a dummy
    // verification when no hash exists) — so neither the response nor its
    // timing reveals whether the email is registered.
    let authenticated = match row.as_ref().and_then(|r| r.password_hash.as_deref()) {
        Some(hash) => passwords::verify_password(&req.password, hash),
        None => {
            passwords::dummy_verify(&req.password);
            false
        }
    };
    if !authenticated {
        state.login_limiter.record_failure(&ip, email);
        return Err(invalid());
    }
    let row = row.expect("authenticated implies a row");

    state.login_limiter.clear(&ip, email);
    let secret = sessions::create_session(&state.pool, row.id).await?;
    let user = User {
        id: row.id,
        email: row.email,
        display_name: row.display_name,
        created_at: row.created_at,
    };
    let jar = jar.add(sessions::session_cookie(secret, state.config.cookie_secure));
    Ok((jar, Json(UserResponse { user })))
}

/// Deletes the session row and clears the cookie. Idempotent.
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> ApiResult<(CookieJar, StatusCode)> {
    if let Some(secret) = jar
        .get(sessions::SESSION_COOKIE)
        .map(|c| c.value().to_string())
    {
        sessions::delete_session(&state.pool, &secret).await?;
    }
    let jar = jar.add(sessions::clear_cookie(state.config.cookie_secure));
    Ok((jar, StatusCode::NO_CONTENT))
}

/// The currently authenticated user (session cookie or Bearer token).
pub async fn me(State(state): State<AppState>, user: CurrentUser) -> ApiResult<Json<UserResponse>> {
    let user = load_user(&state.pool, user.user_id).await?;
    Ok(Json(UserResponse { user }))
}

/// Which sign-in methods this instance offers.
pub async fn providers(State(state): State<AppState>) -> Json<ProvidersResponse> {
    Json(ProvidersResponse {
        password: true,
        github: state.config.github.is_some(),
    })
}

/// Redirects to GitHub's consent screen, setting a CSRF state cookie. 404 when
/// GitHub OAuth isn't configured.
pub async fn github_start(
    State(state): State<AppState>,
    jar: CookieJar,
) -> ApiResult<(CookieJar, Redirect)> {
    let cfg = state
        .config
        .github
        .as_ref()
        .ok_or_else(github_not_configured)?;
    let csrf_state = generate_secret("");
    let redirect_uri = format!(
        "{}/api/auth/github/callback",
        state.config.public_base_url()
    );
    let url = github::authorize_url(cfg, &redirect_uri, &csrf_state);

    let state_cookie = Cookie::build((github::GITHUB_STATE_COOKIE, csrf_state))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(state.config.cookie_secure)
        .max_age(CookieDuration::minutes(10))
        .build();
    Ok((jar.add(state_cookie), Redirect::to(&url)))
}

#[derive(Debug, Deserialize)]
pub struct GithubCallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// Verifies state, exchanges the code, resolves/links/creates the account, and
/// starts a session before redirecting back to the app.
pub async fn github_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<GithubCallbackQuery>,
) -> ApiResult<(CookieJar, Redirect)> {
    let cfg = state
        .config
        .github
        .as_ref()
        .ok_or_else(github_not_configured)?;

    let expected = jar
        .get(github::GITHUB_STATE_COOKIE)
        .map(|c| c.value().to_string());
    let (Some(code), Some(returned), Some(expected)) = (query.code, query.state, expected) else {
        return Err(ApiError::bad_request(
            "github_state_mismatch",
            "missing or invalid OAuth state",
        ));
    };
    if returned != expected {
        return Err(ApiError::bad_request(
            "github_state_mismatch",
            "OAuth state mismatch",
        ));
    }

    let redirect_uri = format!(
        "{}/api/auth/github/callback",
        state.config.public_base_url()
    );
    let access = github::exchange_code(&state.http_client, cfg, &code, &redirect_uri).await?;
    let gh_user = github::fetch_user(&state.http_client, &access).await?;
    let email = github::fetch_primary_email(&state.http_client, &access).await?;

    let user_id = resolve_github_user(&state.pool, gh_user.id, &gh_user.login, &email).await?;
    let secret = sessions::create_session(&state.pool, user_id).await?;

    let clear_state = Cookie::build((github::GITHUB_STATE_COOKIE, ""))
        .path("/")
        .build();
    let jar = jar
        .remove(clear_state)
        .add(sessions::session_cookie(secret, state.config.cookie_secure));
    Ok((jar, Redirect::to(&state.config.public_base_url())))
}

/// Existing GitHub identity → log in; else same verified email → link identity;
/// else create a passwordless account plus identity.
async fn resolve_github_user(
    pool: &PgPool,
    github_id: i64,
    login: &str,
    email: &str,
) -> ApiResult<Uuid> {
    if let Some((user_id,)) =
        sqlx::query_as::<_, (Uuid,)>("SELECT user_id FROM github_identities WHERE github_id = $1")
            .bind(github_id)
            .fetch_optional(pool)
            .await?
    {
        return Ok(user_id);
    }

    if let Some((user_id,)) =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM users WHERE lower(email) = lower($1)")
            .bind(email)
            .fetch_optional(pool)
            .await?
    {
        sqlx::query(
            "INSERT INTO github_identities (github_id, user_id, login) VALUES ($1, $2, $3) \
             ON CONFLICT (github_id) DO NOTHING",
        )
        .bind(github_id)
        .bind(user_id)
        .bind(login)
        .execute(pool)
        .await?;
        return Ok(user_id);
    }

    let mut tx = pool.begin().await?;
    let (user_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO users (email, password_hash, display_name) VALUES ($1, NULL, $2) RETURNING id",
    )
    .bind(email)
    .bind(login)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO github_identities (github_id, user_id, login) VALUES ($1, $2, $3)")
        .bind(github_id)
        .bind(user_id)
        .bind(login)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(user_id)
}

/// Starts a device pairing: returns the one-shot device code and human code.
pub async fn device_code(State(state): State<AppState>) -> ApiResult<Json<DeviceCodeResponse>> {
    let (device_code, user_code) = device::create_device_code(&state.pool).await?;
    let verification_uri = format!("{}/device", state.config.public_base_url());
    Ok(Json(DeviceCodeResponse {
        device_code,
        user_code,
        verification_uri,
        expires_in: device::DEVICE_TTL_MINUTES * 60,
        interval: device::DEVICE_INTERVAL_SECONDS,
    }))
}

/// Polled by the pairing client. Uses the flat RFC 8628 error shape
/// (`{"error": "authorization_pending"}`), not the standard envelope.
pub async fn device_token(
    State(state): State<AppState>,
    Json(req): Json<DeviceTokenRequest>,
) -> Response {
    match device::poll_device(&state.pool, &req.device_code).await {
        Ok(DevicePoll::Approved(created)) => (StatusCode::OK, Json(*created)).into_response(),
        Ok(DevicePoll::Pending) => oauth_error("authorization_pending"),
        Ok(DevicePoll::SlowDown) => oauth_error("slow_down"),
        Ok(DevicePoll::Expired) => oauth_error("expired_token"),
        Ok(DevicePoll::Denied) => oauth_error("access_denied"),
        Err(e) => e.into_response(),
    }
}

fn oauth_error(code: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": code })),
    )
        .into_response()
}

/// Session-authenticated: the user approves (or denies) a device by its code.
pub async fn device_approve(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Json(req): Json<DeviceApproveRequest>,
) -> ApiResult<Json<DeviceApproveResponse>> {
    device::approve_device(&state.pool, user.user_id, &req.user_code, req.approve).await?;
    Ok(Json(DeviceApproveResponse {
        user_code: req.user_code.trim().to_uppercase(),
        approved: req.approve,
    }))
}

/// Loads a user by id for the response body. A resolved `CurrentUser` whose row
/// has since vanished is treated as unauthenticated.
async fn load_user(pool: &PgPool, id: Uuid) -> ApiResult<User> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, email, display_name, created_at FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(Into::into)
    .ok_or_else(|| ApiError::unauthorized("unauthenticated", "account no longer exists"))
}

/// Light structural email check; the `lower(email)` unique index is the real
/// guard against duplicates.
fn validate_email(email: &str) -> ApiResult<()> {
    if email.len() >= 3 && email.contains('@') && !email.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "invalid_email",
            "a valid email is required",
        ))
    }
}

fn github_not_configured() -> ApiError {
    ApiError::not_found(
        "not_found",
        "GitHub OAuth is not configured on this instance",
    )
}
