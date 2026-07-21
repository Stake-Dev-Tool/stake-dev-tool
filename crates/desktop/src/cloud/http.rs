//! Shared connection primitive for the M3 cloud clients
//! ([`super::documents`], [`super::math`], [`super::sse`]).
//!
//! The M1 surface ([`super::api::CloudClient`]) keeps its own `reqwest::Client`;
//! the M3 clients each want slightly different timeout/streaming behaviour, so
//! rather than widen `CloudClient` they share this small helper that resolves
//! the base URL (via [`super::config`]) and the keyring device token (via
//! [`super::auth`]) exactly once.

use std::time::Duration;

use super::auth::load_token;
use super::config;

/// A resolved cloud connection: the HTTP client, the base URL, and the bearer
/// token. Cloning is cheap (`reqwest::Client` is an `Arc` internally).
#[derive(Clone)]
pub struct Conn {
    pub http: reqwest::Client,
    pub base: String,
    pub token: String,
}

impl Conn {
    /// Builds a connection from the keyring-stored device token and the
    /// configured base URL. Returns `None` for the token when signed out so the
    /// caller can surface a "not signed in" error in its own error type.
    pub fn connect() -> anyhow::Result<Option<Self>> {
        let Some(token) = load_token()? else {
            return Ok(None);
        };
        let base = config::get()?;
        // No overall request timeout: a book upload/download can legitimately
        // run for minutes. Only the connect phase is bounded.
        let http = reqwest::Client::builder()
            .user_agent("stake-dev-tool")
            .connect_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Some(Self { http, base, token }))
    }

    /// `{base}{path}` — callers pass a leading-slash absolute path.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// A request builder pre-authenticated with the bearer token.
    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, self.url(path))
            .bearer_auth(&self.token)
    }
}
