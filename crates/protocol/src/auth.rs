//! Wire types for authentication: accounts, sessions, API tokens, the device
//! pairing flow, and provider capability flags.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// Public view of an account. Never carries the password hash.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
}

/// Envelope returned by register / login / me.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct UserResponse {
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Which sign-in methods this instance offers, for the login page to render.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ProvidersResponse {
    pub password: bool,
    pub github: bool,
}

/// A personal API token's metadata. Never carries the secret or its hash.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct TokenInfo {
    pub id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreateTokenRequest {
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_in_days: Option<i64>,
}

/// Response to token creation: the secret is shown exactly once, here.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreatedToken {
    pub token: String,
    pub info: TokenInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct TokensResponse {
    pub tokens: Vec<TokenInfo>,
}

/// Response to `POST /api/auth/device/code` (RFC 8628 shaped).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: i64,
    pub interval: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DeviceTokenRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DeviceApproveRequest {
    pub user_code: String,
    pub approve: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DeviceApproveResponse {
    pub user_code: String,
    pub approved: bool,
}
