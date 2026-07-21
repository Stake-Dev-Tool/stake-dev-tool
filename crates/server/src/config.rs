use std::path::PathBuf;

use thiserror::Error;

/// Default Postgres URL — matches `docker-compose.dev.yml` (port 5433 so it
/// never clashes with a system Postgres on 5432).
const DEFAULT_DATABASE_URL: &str = "postgres://stakedev:stakedev@localhost:5433/stakedev";
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_FS_ROOT: &str = "./data/blobs";
const DEFAULT_S3_REGION: &str = "auto";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("STORAGE_BACKEND must be \"fs\" or \"s3\", got \"{0}\"")]
    InvalidBackend(String),
    #[error("STORAGE_S3_BUCKET is required when STORAGE_BACKEND=s3")]
    MissingBucket,
    #[error("{key} must be a boolean (true/false/1/0), got \"{value}\"")]
    InvalidBool { key: &'static str, value: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageConfig {
    Fs {
        root: PathBuf,
    },
    S3 {
        endpoint: Option<String>,
        bucket: String,
        region: String,
        access_key_id: Option<String>,
        secret_access_key: Option<String>,
        allow_http: bool,
    },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(|key| std::env::var(key).ok())
    }

    /// Reads configuration from an arbitrary key -> value source. Keeps
    /// `from_env` a thin wrapper so the parsing rules are unit-testable without
    /// mutating the process environment.
    fn from_source(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind_addr = get("SERVER_BIND_ADDR").unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
        let database_url = get("DATABASE_URL").unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());

        let backend = get("STORAGE_BACKEND").unwrap_or_else(|| "fs".to_string());
        let storage = match backend.as_str() {
            "fs" => StorageConfig::Fs {
                root: get("STORAGE_FS_ROOT")
                    .unwrap_or_else(|| DEFAULT_FS_ROOT.to_string())
                    .into(),
            },
            "s3" => StorageConfig::S3 {
                endpoint: get("STORAGE_S3_ENDPOINT"),
                bucket: get("STORAGE_S3_BUCKET").ok_or(ConfigError::MissingBucket)?,
                region: get("STORAGE_S3_REGION").unwrap_or_else(|| DEFAULT_S3_REGION.to_string()),
                access_key_id: get("STORAGE_S3_ACCESS_KEY_ID"),
                secret_access_key: get("STORAGE_S3_SECRET_ACCESS_KEY"),
                allow_http: parse_bool(get("STORAGE_S3_ALLOW_HTTP"), "STORAGE_S3_ALLOW_HTTP")?,
            },
            other => return Err(ConfigError::InvalidBackend(other.to_string())),
        };

        Ok(Self {
            bind_addr,
            database_url,
            storage,
        })
    }
}

fn parse_bool(value: Option<String>, key: &'static str) -> Result<bool, ConfigError> {
    match value {
        None => Ok(false),
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" | "" => Ok(false),
            _ => Err(ConfigError::InvalidBool { key, value: v }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn source(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn defaults_to_local_fs_and_dev_postgres() {
        let cfg = Config::from_source(|_| None).unwrap();
        assert_eq!(cfg.bind_addr, DEFAULT_BIND_ADDR);
        assert_eq!(cfg.database_url, DEFAULT_DATABASE_URL);
        assert_eq!(
            cfg.storage,
            StorageConfig::Fs {
                root: PathBuf::from(DEFAULT_FS_ROOT),
            }
        );
    }

    #[test]
    fn parses_s3_backend_with_defaults() {
        let cfg = Config::from_source(source(&[
            ("STORAGE_BACKEND", "s3"),
            ("STORAGE_S3_BUCKET", "stake-dev"),
            ("STORAGE_S3_ENDPOINT", "http://localhost:9000"),
            ("STORAGE_S3_ALLOW_HTTP", "true"),
        ]))
        .unwrap();
        assert_eq!(
            cfg.storage,
            StorageConfig::S3 {
                endpoint: Some("http://localhost:9000".to_string()),
                bucket: "stake-dev".to_string(),
                region: DEFAULT_S3_REGION.to_string(),
                access_key_id: None,
                secret_access_key: None,
                allow_http: true,
            }
        );
    }

    #[test]
    fn s3_without_bucket_is_an_error() {
        let err = Config::from_source(source(&[("STORAGE_BACKEND", "s3")])).unwrap_err();
        assert_eq!(err, ConfigError::MissingBucket);
    }

    #[test]
    fn unknown_backend_is_an_error() {
        let err = Config::from_source(source(&[("STORAGE_BACKEND", "gcs")])).unwrap_err();
        assert_eq!(err, ConfigError::InvalidBackend("gcs".to_string()));
    }

    #[test]
    fn invalid_bool_is_an_error() {
        let err = Config::from_source(source(&[
            ("STORAGE_BACKEND", "s3"),
            ("STORAGE_S3_BUCKET", "stake-dev"),
            ("STORAGE_S3_ALLOW_HTTP", "maybe"),
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            ConfigError::InvalidBool {
                key: "STORAGE_S3_ALLOW_HTTP",
                value: "maybe".to_string(),
            }
        );
    }
}
