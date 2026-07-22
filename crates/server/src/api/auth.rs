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
    ForgotPasswordRequest, LoginRequest, ProvidersResponse, RegisterRequest, ResetPasswordRequest,
    User, UserResponse, VerifyEmailRequest,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use time::Duration as CookieDuration;
use uuid::Uuid;

use crate::AppState;
use crate::auth::device::{self, DevicePoll};
use crate::auth::extract::{ClientIp, CurrentUser, SessionUser};
use crate::auth::{
    discord, email_verify, generate_secret, github, password_reset, passwords, sessions,
};
use crate::config::MailConfig;
use crate::error::{ApiError, ApiResult, is_unique_violation};
use crate::mail;

const MIN_PASSWORD_LEN: usize = 8;

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    email_verified_at: Option<DateTime<Utc>>,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: r.id,
            email: r.email,
            display_name: r.display_name,
            created_at: r.created_at,
            email_verified: r.email_verified_at.is_some(),
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
    // With email configured, new password accounts start unverified and must
    // confirm via the emailed link. Without email (self-host), there is no
    // verification step at all, so the account is born verified.
    let verified_now = state.config.mail.is_none();
    let result = sqlx::query_as::<_, UserRow>(
        "INSERT INTO users (email, password_hash, display_name, email_verified_at) \
         VALUES ($1, $2, $3, CASE WHEN $4 THEN now() ELSE NULL END) \
         RETURNING id, email, display_name, created_at, email_verified_at",
    )
    .bind(email)
    .bind(&password_hash)
    .bind(display_name)
    .bind(verified_now)
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

    // Fire off the verification email (best-effort: a mail failure must not fail
    // signup — the user is signed in and can resend from the dashboard).
    if let Some(mail_cfg) = state.config.mail.as_ref() {
        issue_verification_email(&state, mail_cfg, user.id, &user.email).await;
    }

    let secret = sessions::create_session(&state.pool, user.id).await?;
    let jar = jar.add(sessions::session_cookie(secret, state.config.cookie_secure));
    Ok((jar, Json(UserResponse { user: user.into() })))
}

/// Mints a verification token (awaited, so the record exists before we return)
/// and dispatches the confirmation email in the background — signup and resend
/// must never block on the mail round-trip. Every failure is logged, never
/// propagated (verification can always be resent).
async fn issue_verification_email(
    state: &AppState,
    mail_cfg: &MailConfig,
    user_id: Uuid,
    email: &str,
) {
    let token = match email_verify::create_verification(&state.pool, user_id).await {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(error = %e, "failed to mint email verification token");
            return;
        }
    };
    let url = format!("{}/verify/{}", state.config.public_base_url(), token);
    let html = mail::action_email(
        "Verify your email",
        "Confirm this address to finish setting up your Stake Dev Tool Cloud account.",
        "Verify email",
        &url,
        "If you didn't create this account, you can safely ignore this email.",
    );
    let client = state.http_client.clone();
    let cfg = mail_cfg.clone();
    let to = email.to_string();
    tokio::spawn(async move {
        if let Err(e) = mail::send(&client, &cfg, &to, "Verify your email", &html).await {
            tracing::error!(error = %e, "failed to send verification email");
        }
    });
}

#[derive(sqlx::FromRow)]
struct AuthRow {
    id: Uuid,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    password_hash: Option<String>,
    email_verified_at: Option<DateTime<Utc>>,
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
        "SELECT id, email, display_name, created_at, password_hash, email_verified_at FROM users \
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
        email_verified: row.email_verified_at.is_some(),
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
        discord: state.config.discord.is_some(),
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
        // The provider verified this email, so confirm the account if it wasn't.
        mark_email_verified(pool, user_id).await?;
        return Ok(user_id);
    }

    let mut tx = pool.begin().await?;
    // A provider-created account is verified from birth (the email is confirmed).
    let (user_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO users (email, password_hash, display_name, email_verified_at) \
         VALUES ($1, NULL, $2, now()) RETURNING id",
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

/// Stamps `email_verified_at` if not already set (a provider login has proven
/// the address). Idempotent.
async fn mark_email_verified(pool: &PgPool, user_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE users SET email_verified_at = COALESCE(email_verified_at, now()) WHERE id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

// --- Discord OAuth (mirrors the GitHub flow) --------------------------------

/// Redirects to Discord's consent screen, setting a CSRF state cookie. 404 when
/// Discord OAuth isn't configured.
pub async fn discord_start(
    State(state): State<AppState>,
    jar: CookieJar,
) -> ApiResult<(CookieJar, Redirect)> {
    let cfg = state
        .config
        .discord
        .as_ref()
        .ok_or_else(discord_not_configured)?;
    let csrf_state = generate_secret("");
    let redirect_uri = format!(
        "{}/api/auth/discord/callback",
        state.config.public_base_url()
    );
    let url = discord::authorize_url(cfg, &redirect_uri, &csrf_state);

    let state_cookie = Cookie::build((discord::DISCORD_STATE_COOKIE, csrf_state))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(state.config.cookie_secure)
        .max_age(CookieDuration::minutes(10))
        .build();
    Ok((jar.add(state_cookie), Redirect::to(&url)))
}

#[derive(Debug, Deserialize)]
pub struct DiscordCallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// Verifies state, exchanges the code, requires a verified email, resolves/links/
/// creates the account, and starts a session before redirecting back to the app.
pub async fn discord_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<DiscordCallbackQuery>,
) -> ApiResult<(CookieJar, Redirect)> {
    let cfg = state
        .config
        .discord
        .as_ref()
        .ok_or_else(discord_not_configured)?;

    let expected = jar
        .get(discord::DISCORD_STATE_COOKIE)
        .map(|c| c.value().to_string());
    let (Some(code), Some(returned), Some(expected)) = (query.code, query.state, expected) else {
        return Err(ApiError::bad_request(
            "discord_state_mismatch",
            "missing or invalid OAuth state",
        ));
    };
    if returned != expected {
        return Err(ApiError::bad_request(
            "discord_state_mismatch",
            "OAuth state mismatch",
        ));
    }

    let redirect_uri = format!(
        "{}/api/auth/discord/callback",
        state.config.public_base_url()
    );
    let access = discord::exchange_code(&state.http_client, cfg, &code, &redirect_uri).await?;
    let dc_user = discord::fetch_user(&state.http_client, &access).await?;

    // Accounts are keyed on a verified email; reject an unverified or absent one.
    let email = match (dc_user.email.as_deref(), dc_user.verified) {
        (Some(email), Some(true)) if !email.is_empty() => email.to_string(),
        _ => {
            return Err(ApiError::bad_request(
                "discord_email_unverified",
                "your Discord account has no verified email address",
            ));
        }
    };

    let user_id = resolve_discord_user(&state.pool, &dc_user.id, &dc_user.username, &email).await?;
    let secret = sessions::create_session(&state.pool, user_id).await?;

    let clear_state = Cookie::build((discord::DISCORD_STATE_COOKIE, ""))
        .path("/")
        .build();
    let jar = jar
        .remove(clear_state)
        .add(sessions::session_cookie(secret, state.config.cookie_secure));
    Ok((jar, Redirect::to(&state.config.public_base_url())))
}

/// Existing Discord identity → log in; else same verified email → link identity;
/// else create a passwordless (verified) account plus identity.
async fn resolve_discord_user(
    pool: &PgPool,
    discord_id: &str,
    username: &str,
    email: &str,
) -> ApiResult<Uuid> {
    if let Some((user_id,)) =
        sqlx::query_as::<_, (Uuid,)>("SELECT user_id FROM discord_identities WHERE discord_id = $1")
            .bind(discord_id)
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
            "INSERT INTO discord_identities (discord_id, user_id, username) VALUES ($1, $2, $3) \
             ON CONFLICT (discord_id) DO NOTHING",
        )
        .bind(discord_id)
        .bind(user_id)
        .bind(username)
        .execute(pool)
        .await?;
        mark_email_verified(pool, user_id).await?;
        return Ok(user_id);
    }

    let mut tx = pool.begin().await?;
    let (user_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO users (email, password_hash, display_name, email_verified_at) \
         VALUES ($1, NULL, $2, now()) RETURNING id",
    )
    .bind(email)
    .bind(username)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO discord_identities (discord_id, user_id, username) VALUES ($1, $2, $3)",
    )
    .bind(discord_id)
    .bind(user_id)
    .bind(username)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(user_id)
}

fn discord_not_configured() -> ApiError {
    ApiError::not_found(
        "not_found",
        "Discord OAuth is not configured on this instance",
    )
}

// --- Password reset ---------------------------------------------------------

/// Requests a password-reset email. ALWAYS answers a uniform 200 (regardless of
/// whether the account exists or mail is configured) so it can't be used to
/// probe which emails are registered. Rate-limited per `(ip, email)`.
pub async fn forgot_password(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<ForgotPasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let email = req.email.trim();

    if state.email_limiter.is_blocked(&ip, email) {
        return Err(ApiError::too_many_requests(
            "rate_limited",
            "too many requests; please try again later",
        ));
    }
    state.email_limiter.record_failure(&ip, email);

    let account: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, email FROM users WHERE lower(email) = lower($1)")
            .bind(email)
            .fetch_optional(&state.pool)
            .await?;

    match (account, state.config.mail.as_ref()) {
        (Some((user_id, user_email)), Some(mail_cfg)) => {
            // Best-effort: mint a token and email the link. Any failure is logged,
            // never surfaced (surfacing would leak that the account exists).
            match password_reset::create_reset(&state.pool, user_id).await {
                Ok(token) => {
                    let url = format!("{}/reset/{}", state.config.public_base_url(), token);
                    let html = mail::action_email(
                        "Reset your password",
                        "We received a request to reset the password for your Stake Dev Tool \
                         Cloud account. This link expires in one hour.",
                        "Reset password",
                        &url,
                        "If you didn't request a password reset, you can safely ignore this email.",
                    );
                    // Send in the background so the response can't be timed to
                    // reveal whether the account exists.
                    let client = state.http_client.clone();
                    let cfg = mail_cfg.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            mail::send(&client, &cfg, &user_email, "Reset your password", &html)
                                .await
                        {
                            tracing::error!(error = %e, "failed to send password reset email");
                        }
                    });
                }
                Err(e) => tracing::error!(error = %e, "failed to mint password reset token"),
            }
        }
        (Some(_), None) => {
            tracing::warn!("password reset requested but email is not configured; skipping send");
        }
        (None, _) => {
            // No such account — do nothing, but answer identically.
        }
    }

    Ok(Json(json!({ "ok": true })))
}

/// Redeems a reset token and sets a new password. GitHub/Discord-only accounts
/// (NULL password_hash) gain a usable password this way — intended.
pub async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetPasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.password.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::unprocessable(
            "weak_password",
            format!("password must be at least {MIN_PASSWORD_LEN} characters"),
        ));
    }
    let hash = passwords::hash_password(&req.password)?;
    let ok = password_reset::consume_reset(&state.pool, req.token.trim(), &hash).await?;
    if !ok {
        return Err(ApiError::bad_request(
            "invalid_token",
            "this password reset link is invalid or has expired",
        ));
    }
    Ok(Json(json!({ "ok": true })))
}

// --- Email verification -----------------------------------------------------

/// Redeems a verification token, confirming the account's email. Uniform 400
/// `invalid_token` for an unknown/expired/used token.
pub async fn verify_email(
    State(state): State<AppState>,
    Json(req): Json<VerifyEmailRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let ok = email_verify::consume_verification(&state.pool, req.token.trim()).await?;
    if !ok {
        return Err(ApiError::bad_request(
            "invalid_token",
            "this verification link is invalid or has expired",
        ));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Session-authenticated: resends the verification email. A no-op 200 when the
/// account is already verified or mail is unconfigured. Rate-limited per
/// `(ip, email)` like forgot-password.
pub async fn resend_verification(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    SessionUser(user): SessionUser,
) -> ApiResult<Json<serde_json::Value>> {
    let row: Option<(String, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT email, email_verified_at FROM users WHERE id = $1")
            .bind(user.user_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((email, verified_at)) = row else {
        return Err(ApiError::unauthorized(
            "unauthenticated",
            "account no longer exists",
        ));
    };

    // No-op success when there's nothing to do.
    let Some(mail_cfg) = state.config.mail.as_ref() else {
        return Ok(Json(json!({ "ok": true })));
    };
    if verified_at.is_some() {
        return Ok(Json(json!({ "ok": true })));
    }

    if state.email_limiter.is_blocked(&ip, &email) {
        return Err(ApiError::too_many_requests(
            "rate_limited",
            "too many requests; please try again later",
        ));
    }
    state.email_limiter.record_failure(&ip, &email);
    issue_verification_email(&state, mail_cfg, user.user_id, &email).await;
    Ok(Json(json!({ "ok": true })))
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
        "SELECT id, email, display_name, created_at, email_verified_at FROM users WHERE id = $1",
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
