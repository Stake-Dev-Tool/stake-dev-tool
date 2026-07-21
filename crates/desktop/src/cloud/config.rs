//! Cloud base-URL configuration.
//!
//! Resolution precedence, highest first:
//! 1. the `STAKE_DEV_CLOUD_URL` environment variable,
//! 2. `cloud.json` next to the app's other config files (`profiles.json`,
//!    `teams.json`) under `%LOCALAPPDATA%/stake-dev-tool`,
//! 3. the built-in default (the server's default local bind address).
//!
//! Self-hosters set either the env var or, via [`set`], the config file so the
//! desktop app talks to their instance instead of the hosted one.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Environment variable that overrides the configured / default base URL.
pub const ENV_VAR: &str = "STAKE_DEV_CLOUD_URL";

/// Built-in default: the server's default bind address (`crates/server`
/// `SERVER_BIND_ADDR`). Fine for local dev; self-hosters override it.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

/// On-disk shape of `cloud.json`. Uses the same camelCase + serde convention as
/// the app's other config files (`profiles.json`, `teams.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct CloudConfigFile {
    #[serde(default, rename = "baseUrl", skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
}

fn config_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("could not resolve local data dir"))?
        .join("stake-dev-tool");
    Ok(dir.join("cloud.json"))
}

fn load_file() -> Result<CloudConfigFile> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(CloudConfigFile::default());
    }
    let bytes = std::fs::read(&path).context("read cloud.json")?;
    serde_json::from_slice(&bytes).context("parse cloud.json")
}

/// Canonicalizes a base URL: trims surrounding whitespace and any trailing
/// slash so callers can always append `/api/...`. Blank input becomes `None`.
fn normalize(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Pure precedence resolver, split out so it can be unit-tested with injected
/// sources (mirrors `crates/server/src/config.rs`'s `from_source`). Blank
/// values fall through to the next source.
fn resolve(env: Option<String>, file: Option<String>) -> String {
    env.as_deref()
        .and_then(normalize)
        .or_else(|| file.as_deref().and_then(normalize))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// The effective cloud base URL, following the precedence documented above.
pub fn get() -> Result<String> {
    let env = std::env::var(ENV_VAR).ok();
    let file = load_file()?.base_url;
    Ok(resolve(env, file))
}

/// Persists `url` to `cloud.json` and returns the normalized value stored. Note
/// that a set `STAKE_DEV_CLOUD_URL` still wins on the next [`get`].
pub fn set(url: &str) -> Result<String> {
    let normalized = normalize(url).ok_or_else(|| anyhow!("base URL must not be empty"))?;
    // Reject anything that isn't an http(s) URL so we never persist garbage that
    // would then fail every request made against it.
    let parsed = url::Url::parse(&normalized).context("base URL is not a valid URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(anyhow!("base URL must use http or https"));
    }

    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create config dir")?;
    }
    let file = CloudConfigFile {
        base_url: Some(normalized.clone()),
    };
    let bytes = serde_json::to_vec_pretty(&file).context("serialize cloud.json")?;
    std::fs::write(&path, bytes).context("write cloud.json")?;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_takes_precedence_over_file() {
        assert_eq!(
            resolve(
                Some("https://env.example.com".into()),
                Some("https://file.example.com".into()),
            ),
            "https://env.example.com",
        );
    }

    #[test]
    fn file_used_when_env_absent() {
        assert_eq!(
            resolve(None, Some("https://file.example.com".into())),
            "https://file.example.com",
        );
    }

    #[test]
    fn default_when_neither_source_is_set() {
        assert_eq!(resolve(None, None), DEFAULT_BASE_URL);
    }

    #[test]
    fn blank_sources_fall_through_to_the_next() {
        // A blank env var falls back to the file...
        assert_eq!(
            resolve(Some("   ".into()), Some("https://file.example.com".into())),
            "https://file.example.com",
        );
        // ...and a blank file falls back to the default.
        assert_eq!(
            resolve(Some(String::new()), Some("  ".into())),
            DEFAULT_BASE_URL
        );
    }

    #[test]
    fn trailing_slashes_are_trimmed() {
        assert_eq!(
            resolve(Some("https://x.example.com/".into()), None),
            "https://x.example.com",
        );
    }
}
