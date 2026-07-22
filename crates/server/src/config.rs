use std::net::IpAddr;
use std::path::PathBuf;

use thiserror::Error;

/// Default Postgres URL — matches `docker-compose.dev.yml` (port 5433 so it
/// never clashes with a system Postgres on 5432).
const DEFAULT_DATABASE_URL: &str = "postgres://stakedev:stakedev@localhost:5433/stakedev";
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_FS_ROOT: &str = "./data/blobs";
const DEFAULT_S3_REGION: &str = "auto";
/// Upper bound on a single uploaded blob (8 GiB). Beyond it a blob PUT is
/// rejected with `413 payload_too_large`.
const DEFAULT_MAX_BLOB_BYTES: u64 = 8_589_934_592;
/// Byte budget for the on-disk materialized-revision cache used by the
/// multi-tenant LGS host (20 GiB). Least-recently-used completed revision
/// directories are evicted once the total exceeds this. See `lgs_host`.
const DEFAULT_MATH_CACHE_BYTES: u64 = 21_474_836_480;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("STORAGE_BACKEND must be \"fs\" or \"s3\", got \"{0}\"")]
    InvalidBackend(String),
    #[error("STORAGE_S3_BUCKET is required when STORAGE_BACKEND=s3")]
    MissingBucket,
    #[error("{key} must be a boolean (true/false/1/0), got \"{value}\"")]
    InvalidBool { key: &'static str, value: String },
    #[error("{key} must be a non-negative integer, got \"{value}\"")]
    InvalidU64 { key: &'static str, value: String },
    #[error("SERVER_TRUSTED_PROXIES entry is not a valid IP or CIDR: \"{0}\"")]
    InvalidCidr(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub storage: StorageConfig,
    /// `Secure` flag on the session cookie. Off for local http dev; must be on
    /// behind TLS in production.
    pub cookie_secure: bool,
    /// Externally reachable base URL (e.g. `https://app.example.com`). Backs the
    /// invite/device URLs and the GitHub OAuth redirect. Falls back to the bind
    /// address when unset.
    pub public_url: Option<String>,
    /// Present only when GitHub OAuth is fully configured (client id, secret,
    /// and a public URL). Absent → the GitHub routes 404.
    pub github: Option<GithubConfig>,
    /// Present only when Discord OAuth is fully configured (client id, secret,
    /// and a public URL). Absent → the Discord routes 404. Mirrors [`GithubConfig`].
    pub discord: Option<DiscordConfig>,
    /// Present only when transactional email is configured (Resend API key and a
    /// `MAIL_FROM` address). Absent → password-reset emails are skipped (the
    /// forgot-password endpoint still answers 200, logging a warning). See
    /// [`MailConfig`].
    pub mail: Option<MailConfig>,
    /// Present only when Stripe billing is fully configured (secret key, webhook
    /// secret, and all five price ids). Absent → billing routes 404, every
    /// workspace resolves to unlimited, and no quota check ever fires (the
    /// permanent state on self-hosted instances). See [`StripeConfig`].
    pub stripe: Option<StripeConfig>,
    /// Explicit dashboard build directory (`SERVER_WEB_DIR`). When unset,
    /// [`Config::resolve_web_dir`] probes the standard locations.
    pub web_dir: Option<PathBuf>,
    /// Maximum bytes accepted for a single blob upload (`STORAGE_MAX_BLOB_BYTES`,
    /// default 8 GiB). A larger streamed body is aborted with `413`.
    pub storage_max_blob_bytes: u64,
    /// Byte budget for the multi-tenant LGS host's on-disk materialized-revision
    /// cache (`SERVER_MATH_CACHE_BYTES`, default 20 GiB). Completed revision
    /// directories are LRU-evicted (by `.complete` marker mtime) past this.
    pub server_math_cache_bytes: u64,
    /// Optional per-tenant decompressed-books cap (`SERVER_TENANT_BOOKS_CAP_BYTES`)
    /// applied to every hosted tenant via `TenantRegistry::set_tenant_cap`.
    /// `None` (the default) leaves tenants uncapped, sharing the process-global
    /// books budget. A billing plan can override this per workspace later.
    pub server_tenant_books_cap_bytes: Option<u64>,
    /// Registrable base of the wildcard share domains (`SERVER_PLAY_DOMAIN`,
    /// e.g. `play.stakedevtool.com`). Requests whose Host is
    /// `<slug>.<play_domain>` are dispatched to the share router (M5); unset
    /// disables host-based share serving entirely.
    pub play_domain: Option<String>,
    /// Instance-operator email allowlist (`SERVER_ADMIN_EMAILS`, comma-separated,
    /// trimmed + lowercased, empties dropped). A user whose email is in this list
    /// is an admin even without the `users.is_admin` flag — this is how the FIRST
    /// admin is bootstrapped, before any admin exists to set the flag via SQL.
    /// Empty/unset means the allowlist grants no one.
    pub admin_emails: Vec<String>,
    /// Reverse-proxy networks whose forwarding headers we trust
    /// (`SERVER_TRUSTED_PROXIES`, comma-separated IPs or CIDRs, e.g.
    /// `127.0.0.1/32,::1/128`). `X-Forwarded-For` / `X-Real-IP` are honored for
    /// rate-limit keying ONLY when the direct socket peer falls inside one of
    /// these networks; from any other peer the headers are ignored and the socket
    /// address is used. Empty (the default) trusts no proxy — the safe default
    /// for a directly-exposed server. Behind our Caddy reverse proxy on the same
    /// host, production sets `127.0.0.1/32,::1/128`.
    pub trusted_proxies: TrustedProxies,
}

/// A parsed set of trusted reverse-proxy networks (see [`Config::trusted_proxies`]).
///
/// Backs the `X-Forwarded-For` / `X-Real-IP` trust decision in
/// [`crate::auth::extract`]: forwarding headers are honored only when the direct
/// socket peer is [`TrustedProxies::contains`]. Empty by default, so a
/// directly-exposed server never honors a spoofable header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrustedProxies {
    nets: Vec<Cidr>,
}

/// One CIDR network in the trusted-proxy allowlist. A bare IP parses as a host
/// route (`/32` for IPv4, `/128` for IPv6).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cidr {
    /// The network address exactly as parsed (host bits are masked at compare
    /// time, so an un-normalized entry like `10.0.0.5/8` still matches correctly).
    addr: IpAddr,
    prefix: u8,
}

impl TrustedProxies {
    /// Parse a comma-separated list of IPs / CIDRs. Whitespace is trimmed and
    /// empty entries dropped; a malformed entry is a hard [`ConfigError`] so a
    /// typo surfaces at startup rather than silently trusting nothing.
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        let mut nets = Vec::new();
        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            nets.push(Cidr::parse(entry)?);
        }
        Ok(Self { nets })
    }

    /// True when no proxy is trusted (forwarding headers are never honored).
    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    /// True when `ip` falls inside any trusted network. An IPv4-mapped IPv6 peer
    /// (`::ffff:a.b.c.d`) is compared as its IPv4 form.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = normalize_ip(ip);
        self.nets.iter().any(|net| net.contains(ip))
    }
}

impl Cidr {
    fn parse(entry: &str) -> Result<Self, ConfigError> {
        let invalid = || ConfigError::InvalidCidr(entry.to_string());
        let (addr, prefix) = match entry.split_once('/') {
            Some((ip, p)) => {
                let addr: IpAddr = ip.trim().parse().map_err(|_| invalid())?;
                let max = if addr.is_ipv4() { 32 } else { 128 };
                let prefix: u8 = p.trim().parse().map_err(|_| invalid())?;
                if prefix > max {
                    return Err(invalid());
                }
                (addr, prefix)
            }
            None => {
                let addr: IpAddr = entry.parse().map_err(|_| invalid())?;
                let prefix = if addr.is_ipv4() { 32 } else { 128 };
                (addr, prefix)
            }
        };
        Ok(Self { addr, prefix })
    }

    /// True when `ip` (already normalized) shares this network's high `prefix`
    /// bits. Mismatched address families never match.
    fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(addr)) => {
                mask_v4(u32::from(net), self.prefix) == mask_v4(u32::from(addr), self.prefix)
            }
            (IpAddr::V6(net), IpAddr::V6(addr)) => {
                mask_v6(u128::from(net), self.prefix) == mask_v6(u128::from(addr), self.prefix)
            }
            _ => false,
        }
    }
}

/// Fold an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) down to its IPv4 form so a
/// dual-stack listener's mapped peer matches an IPv4 CIDR entry.
fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        other => other,
    }
}

/// Zero the low `32 - prefix` bits of an IPv4 address. `prefix == 0` masks
/// everything (a shift by 32 would be undefined, so it is special-cased).
fn mask_v4(bits: u32, prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        bits & (u32::MAX << (32 - prefix))
    }
}

/// Zero the low `128 - prefix` bits of an IPv6 address. `prefix == 0` masks
/// everything (a shift by 128 would be undefined, so it is special-cased).
fn mask_v6(bits: u128, prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        bits & (u128::MAX << (128 - prefix))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubConfig {
    pub client_id: String,
    pub client_secret: String,
}

/// Discord OAuth credentials. Constructed only when the client id, the client
/// secret, and a public URL (for the redirect) are all present — the exact
/// [`GithubConfig`] pattern, so its mere existence means Discord login is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordConfig {
    pub client_id: String,
    pub client_secret: String,
}

/// Transactional-email (Resend) configuration. Constructed only when both the
/// API key and a `from` address are present, so its existence means email
/// sending is enabled. Used to send password-reset links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailConfig {
    /// Resend API key (`RESEND_API_KEY`, `re_…`), sent as the Bearer token.
    pub resend_api_key: String,
    /// `From` header (`MAIL_FROM`, e.g. `Stake Dev Tool Cloud <no-reply@stakedevtool.com>`).
    pub from: String,
}

/// Fully-resolved Stripe billing configuration. Only constructed when the secret
/// key, webhook secret, and all five price ids are present (the GitHub
/// optional-block pattern), so its mere existence means billing is active. Test
/// vs live is chosen purely by which `STRIPE_SECRET_KEY`/price ids are supplied —
/// the API host is always `api.stripe.com`, so there is no server/env selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripeConfig {
    /// Secret API key for Stripe (`STRIPE_SECRET_KEY`, `sk_test_…`/`sk_live_…`),
    /// sent as the Bearer token on the checkout call.
    pub secret_key: String,
    /// Webhook signing secret (`STRIPE_WEBHOOK_SECRET`, `whsec_…`). Used verbatim
    /// as the raw-ASCII HMAC key when verifying the `Stripe-Signature` header
    /// (Stripe does NOT base64-decode it, unlike Standard Webhooks).
    pub webhook_secret: String,
    /// Per-seat price (Stripe graduated tiers: €3 first seat, €2 each additional).
    /// The subscription quantity is the seat count.
    pub price_seat_monthly: String,
    pub price_seat_yearly: String,
    /// Quantity-based price for the storage add-on (one unit = +10 GiB).
    pub price_storage: String,
    /// Optional Stripe Customer Portal configuration id
    /// (`STRIPE_PORTAL_CONFIGURATION`, `bpc_…`). When set, portal sessions are
    /// created against this configuration; when `None`, Stripe falls back to the
    /// account's default portal configuration. Purely optional — its absence does
    /// not gate billing.
    pub portal_configuration: Option<String>,
}

impl Config {
    /// Base URL for building externally shared links (invites, device pairing,
    /// OAuth redirects). Uses `SERVER_PUBLIC_URL` when set, otherwise assumes
    /// plain http on the bind address (fine for local dev).
    pub fn public_base_url(&self) -> String {
        self.public_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", self.bind_addr))
    }

    /// The bare host of `SERVER_PUBLIC_URL` — scheme, userinfo, port, and path
    /// stripped, lowercased (e.g. `app.example.com` for
    /// `https://app.example.com/`). Backs two custom-domain rules: a workspace
    /// may not claim a play domain that equals or nests under the dashboard's own
    /// host, and the Host-dispatch layer skips the custom-domain DB probe for
    /// ordinary dashboard traffic addressed to this host. `None` when no public
    /// URL is configured.
    pub fn app_host(&self) -> Option<String> {
        let url = self.public_url.as_deref()?;
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        // Drop any `user:pass@` userinfo, then the `:port`.
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or("");
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        (!host.is_empty()).then_some(host)
    }

    /// Locates the dashboard's static build (`web/build`). An explicit
    /// `SERVER_WEB_DIR` wins even if the path is missing (so a typo surfaces as
    /// a warning instead of silently falling back); otherwise probes the
    /// packaged location next to the binary's cwd, then the in-repo build for
    /// `cargo run` from the workspace root.
    pub fn resolve_web_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = &self.web_dir {
            return Some(dir.clone());
        }
        let candidates = [
            PathBuf::from("./web/build"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/build"),
        ];
        candidates
            .into_iter()
            .find(|c| c.join("index.html").exists())
    }
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

        let storage_max_blob_bytes =
            parse_u64(get("STORAGE_MAX_BLOB_BYTES"), "STORAGE_MAX_BLOB_BYTES")?
                .unwrap_or(DEFAULT_MAX_BLOB_BYTES);

        let server_math_cache_bytes =
            parse_u64(get("SERVER_MATH_CACHE_BYTES"), "SERVER_MATH_CACHE_BYTES")?
                .unwrap_or(DEFAULT_MATH_CACHE_BYTES);
        // Optional: `None` (unset/empty) leaves every tenant uncapped.
        let server_tenant_books_cap_bytes = parse_u64(
            get("SERVER_TENANT_BOOKS_CAP_BYTES"),
            "SERVER_TENANT_BOOKS_CAP_BYTES",
        )?;

        let cookie_secure = parse_bool(get("SERVER_COOKIE_SECURE"), "SERVER_COOKIE_SECURE")?;
        // Trailing slashes are trimmed so callers can always append "/path".
        let public_url = get("SERVER_PUBLIC_URL").map(|s| s.trim_end_matches('/').to_string());

        // GitHub OAuth stays disabled unless the id, the secret, and a public
        // URL (for the redirect) are all present.
        let github = match (
            get("GITHUB_CLIENT_ID"),
            get("GITHUB_CLIENT_SECRET"),
            public_url.is_some(),
        ) {
            (Some(client_id), Some(client_secret), true) => Some(GithubConfig {
                client_id,
                client_secret,
            }),
            _ => None,
        };

        // Discord OAuth follows the same rule: id, secret, and a public URL.
        let discord = match (
            non_empty(get("DISCORD_CLIENT_ID")),
            non_empty(get("DISCORD_CLIENT_SECRET")),
            public_url.is_some(),
        ) {
            (Some(client_id), Some(client_secret), true) => Some(DiscordConfig {
                client_id,
                client_secret,
            }),
            _ => None,
        };

        // Email sending stays disabled unless both the Resend API key and a
        // `from` address are present.
        let mail = match (
            non_empty(get("RESEND_API_KEY")),
            non_empty(get("MAIL_FROM")),
        ) {
            (Some(resend_api_key), Some(from)) => Some(MailConfig {
                resend_api_key,
                from,
            }),
            _ => None,
        };

        // Stripe billing stays disabled unless the secret key, the webhook
        // secret, and all three price ids are present. Any one missing (or blank)
        // leaves the whole block off — self-hosters run unlimited.
        let stripe = match (
            non_empty(get("STRIPE_SECRET_KEY")),
            non_empty(get("STRIPE_WEBHOOK_SECRET")),
            non_empty(get("STRIPE_PRICE_SEAT_MONTHLY")),
            non_empty(get("STRIPE_PRICE_SEAT_YEARLY")),
            non_empty(get("STRIPE_PRICE_STORAGE")),
        ) {
            (
                Some(secret_key),
                Some(webhook_secret),
                Some(price_seat_monthly),
                Some(price_seat_yearly),
                Some(price_storage),
            ) => Some(StripeConfig {
                secret_key,
                webhook_secret,
                price_seat_monthly,
                price_seat_yearly,
                price_storage,
                // Optional: absent/blank → account default portal configuration.
                portal_configuration: non_empty(get("STRIPE_PORTAL_CONFIGURATION")),
            }),
            _ => None,
        };

        Ok(Self {
            bind_addr,
            database_url,
            storage,
            cookie_secure,
            public_url,
            github,
            discord,
            mail,
            stripe,
            web_dir: get("SERVER_WEB_DIR").map(PathBuf::from),
            storage_max_blob_bytes,
            server_math_cache_bytes,
            server_tenant_books_cap_bytes,
            play_domain: get("SERVER_PLAY_DOMAIN")
                .map(|s| s.trim_matches(['.', ' ']).to_ascii_lowercase())
                .filter(|s| !s.is_empty()),
            admin_emails: parse_admin_emails(get("SERVER_ADMIN_EMAILS")),
            trusted_proxies: TrustedProxies::parse(
                &get("SERVER_TRUSTED_PROXIES").unwrap_or_default(),
            )?,
        })
    }
}

/// Splits `SERVER_ADMIN_EMAILS` on commas, trimming and lowercasing each entry
/// and dropping empties. Unset/empty yields an empty list.
fn parse_admin_emails(value: Option<String>) -> Vec<String> {
    value
        .map(|s| {
            s.split(',')
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect()
        })
        .unwrap_or_default()
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

/// Treats an unset or whitespace-only env var as absent, so a stray `KEY=` line
/// never half-enables an optional block.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// Parses an optional unsigned integer env var. `None`/empty → `Ok(None)` so the
/// caller can apply its default; anything non-numeric is a hard error.
fn parse_u64(value: Option<String>, key: &'static str) -> Result<Option<u64>, ConfigError> {
    match value {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => v
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| ConfigError::InvalidU64 { key, value: v }),
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

    #[test]
    fn max_blob_bytes_defaults_and_parses() {
        let cfg = Config::from_source(|_| None).unwrap();
        assert_eq!(cfg.storage_max_blob_bytes, DEFAULT_MAX_BLOB_BYTES);
        let cfg = Config::from_source(source(&[("STORAGE_MAX_BLOB_BYTES", "1048576")])).unwrap();
        assert_eq!(cfg.storage_max_blob_bytes, 1_048_576);
        let err = Config::from_source(source(&[("STORAGE_MAX_BLOB_BYTES", "huge")])).unwrap_err();
        assert_eq!(
            err,
            ConfigError::InvalidU64 {
                key: "STORAGE_MAX_BLOB_BYTES",
                value: "huge".to_string(),
            }
        );
    }

    #[test]
    fn math_cache_bytes_defaults_and_parses() {
        let cfg = Config::from_source(|_| None).unwrap();
        assert_eq!(cfg.server_math_cache_bytes, DEFAULT_MATH_CACHE_BYTES);
        assert_eq!(cfg.server_tenant_books_cap_bytes, None);

        let cfg = Config::from_source(source(&[
            ("SERVER_MATH_CACHE_BYTES", "1048576"),
            ("SERVER_TENANT_BOOKS_CAP_BYTES", "2097152"),
        ]))
        .unwrap();
        assert_eq!(cfg.server_math_cache_bytes, 1_048_576);
        assert_eq!(cfg.server_tenant_books_cap_bytes, Some(2_097_152));

        let err = Config::from_source(source(&[("SERVER_MATH_CACHE_BYTES", "lots")])).unwrap_err();
        assert_eq!(
            err,
            ConfigError::InvalidU64 {
                key: "SERVER_MATH_CACHE_BYTES",
                value: "lots".to_string(),
            }
        );
    }

    #[test]
    fn cookie_secure_defaults_off_and_parses() {
        let cfg = Config::from_source(|_| None).unwrap();
        assert!(!cfg.cookie_secure);
        let cfg = Config::from_source(source(&[("SERVER_COOKIE_SECURE", "true")])).unwrap();
        assert!(cfg.cookie_secure);
    }

    #[test]
    fn github_needs_id_secret_and_public_url() {
        // Missing the public URL keeps GitHub disabled.
        let cfg = Config::from_source(source(&[
            ("GITHUB_CLIENT_ID", "id"),
            ("GITHUB_CLIENT_SECRET", "secret"),
        ]))
        .unwrap();
        assert_eq!(cfg.github, None);

        let cfg = Config::from_source(source(&[
            ("GITHUB_CLIENT_ID", "id"),
            ("GITHUB_CLIENT_SECRET", "secret"),
            ("SERVER_PUBLIC_URL", "https://app.example.com/"),
        ]))
        .unwrap();
        assert_eq!(
            cfg.github,
            Some(GithubConfig {
                client_id: "id".to_string(),
                client_secret: "secret".to_string(),
            })
        );
        // The trailing slash is trimmed so appended paths stay clean.
        assert_eq!(cfg.public_base_url(), "https://app.example.com");
    }

    #[test]
    fn discord_needs_id_secret_and_public_url() {
        // Missing the public URL keeps Discord disabled.
        let cfg = Config::from_source(source(&[
            ("DISCORD_CLIENT_ID", "id"),
            ("DISCORD_CLIENT_SECRET", "secret"),
        ]))
        .unwrap();
        assert_eq!(cfg.discord, None);

        let cfg = Config::from_source(source(&[
            ("DISCORD_CLIENT_ID", "id"),
            ("DISCORD_CLIENT_SECRET", "secret"),
            ("SERVER_PUBLIC_URL", "https://app.example.com/"),
        ]))
        .unwrap();
        assert_eq!(
            cfg.discord,
            Some(DiscordConfig {
                client_id: "id".to_string(),
                client_secret: "secret".to_string(),
            })
        );
    }

    #[test]
    fn mail_needs_key_and_from() {
        // Neither set → disabled.
        let cfg = Config::from_source(|_| None).unwrap();
        assert_eq!(cfg.mail, None);

        // Only one set → still disabled.
        let cfg = Config::from_source(source(&[("RESEND_API_KEY", "re_x")])).unwrap();
        assert_eq!(cfg.mail, None);

        // Both set → enabled.
        let cfg = Config::from_source(source(&[
            ("RESEND_API_KEY", "re_x"),
            ("MAIL_FROM", "Cloud <no-reply@example.com>"),
        ]))
        .unwrap();
        assert_eq!(
            cfg.mail,
            Some(MailConfig {
                resend_api_key: "re_x".to_string(),
                from: "Cloud <no-reply@example.com>".to_string(),
            })
        );

        // A blank value counts as absent.
        let cfg = Config::from_source(source(&[
            ("RESEND_API_KEY", "   "),
            ("MAIL_FROM", "Cloud <no-reply@example.com>"),
        ]))
        .unwrap();
        assert_eq!(cfg.mail, None);
    }

    /// The full set of env vars that enable Stripe billing.
    const STRIPE_ENV: [(&str, &str); 5] = [
        ("STRIPE_SECRET_KEY", "sk_test_xxx"),
        ("STRIPE_WEBHOOK_SECRET", "whsec_abc"),
        ("STRIPE_PRICE_SEAT_MONTHLY", "price_seat_m"),
        ("STRIPE_PRICE_SEAT_YEARLY", "price_seat_y"),
        ("STRIPE_PRICE_STORAGE", "price_storage"),
    ];

    #[test]
    fn stripe_disabled_by_default_and_unlimited() {
        let cfg = Config::from_source(|_| None).unwrap();
        assert_eq!(cfg.stripe, None);
    }

    #[test]
    fn stripe_enables_only_when_every_var_is_present() {
        // The full set enables it.
        let cfg = Config::from_source(source(&STRIPE_ENV)).unwrap();
        let stripe = cfg.stripe.expect("billing enabled");
        assert_eq!(stripe.secret_key, "sk_test_xxx");
        assert_eq!(stripe.webhook_secret, "whsec_abc");
        assert_eq!(stripe.price_seat_monthly, "price_seat_m");
        assert_eq!(stripe.price_seat_yearly, "price_seat_y");
        assert_eq!(stripe.price_storage, "price_storage");

        // Dropping any single required var disables the whole block.
        for (i, missing) in STRIPE_ENV.iter().enumerate() {
            let subset: Vec<(&str, &str)> = STRIPE_ENV
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, kv)| *kv)
                .collect();
            let cfg = Config::from_source(source(&subset)).unwrap();
            assert_eq!(
                cfg.stripe, None,
                "missing {} should disable billing",
                missing.0
            );
        }

        // A present-but-empty var counts as absent.
        let mut with_blank = STRIPE_ENV.to_vec();
        with_blank[0] = ("STRIPE_SECRET_KEY", "   ");
        assert_eq!(
            Config::from_source(source(&with_blank)).unwrap().stripe,
            None
        );
    }

    #[test]
    fn app_host_strips_scheme_port_and_path() {
        let cfg = Config::from_source(|_| None).unwrap();
        assert_eq!(cfg.app_host(), None);

        let cfg = Config::from_source(source(&[(
            "SERVER_PUBLIC_URL",
            "https://App.Example.com:8443/x",
        )]))
        .unwrap();
        assert_eq!(cfg.app_host().as_deref(), Some("app.example.com"));

        let cfg =
            Config::from_source(source(&[("SERVER_PUBLIC_URL", "http://localhost:8080")])).unwrap();
        assert_eq!(cfg.app_host().as_deref(), Some("localhost"));
    }

    #[test]
    fn admin_emails_parse_trimmed_lowercased_and_deduped_of_empties() {
        // Unset → no admins.
        let cfg = Config::from_source(|_| None).unwrap();
        assert!(cfg.admin_emails.is_empty());

        // Comma-separated, trimmed, lowercased; blank entries dropped.
        let cfg = Config::from_source(source(&[(
            "SERVER_ADMIN_EMAILS",
            " Owner@Example.com , ops@example.com ,, ",
        )]))
        .unwrap();
        assert_eq!(
            cfg.admin_emails,
            vec![
                "owner@example.com".to_string(),
                "ops@example.com".to_string()
            ]
        );

        // A whitespace-only value grants no one.
        let cfg = Config::from_source(source(&[("SERVER_ADMIN_EMAILS", "   ")])).unwrap();
        assert!(cfg.admin_emails.is_empty());
    }

    #[test]
    fn trusted_proxies_default_empty_trusts_no_one() {
        let cfg = Config::from_source(|_| None).unwrap();
        assert!(cfg.trusted_proxies.is_empty());
        assert!(!cfg.trusted_proxies.contains("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn trusted_proxies_parses_ips_and_cidrs() {
        let cfg = Config::from_source(source(&[(
            "SERVER_TRUSTED_PROXIES",
            " 127.0.0.1 , 10.0.0.0/8 , ::1 , 2001:db8::/32 ,, ",
        )]))
        .unwrap();
        let tp = &cfg.trusted_proxies;
        assert!(!tp.is_empty());
        // Bare IPv4 host route.
        assert!(tp.contains("127.0.0.1".parse().unwrap()));
        assert!(!tp.contains("127.0.0.2".parse().unwrap()));
        // IPv4 CIDR range.
        assert!(tp.contains("10.1.2.3".parse().unwrap()));
        assert!(tp.contains("10.255.255.255".parse().unwrap()));
        assert!(!tp.contains("11.0.0.1".parse().unwrap()));
        // Bare IPv6 host route.
        assert!(tp.contains("::1".parse().unwrap()));
        // IPv6 CIDR range.
        assert!(tp.contains("2001:db8::dead:beef".parse().unwrap()));
        assert!(!tp.contains("2001:dead::1".parse().unwrap()));
        // Mismatched family never matches.
        assert!(!tp.contains("192.168.0.1".parse().unwrap()));
    }

    #[test]
    fn trusted_proxies_matches_ipv4_mapped_ipv6_peer() {
        // A dual-stack listener may report an IPv4 peer as ::ffff:a.b.c.d; it must
        // still match an IPv4 CIDR entry.
        let cfg =
            Config::from_source(source(&[("SERVER_TRUSTED_PROXIES", "127.0.0.1/32")])).unwrap();
        assert!(
            cfg.trusted_proxies
                .contains("::ffff:127.0.0.1".parse().unwrap())
        );
    }

    #[test]
    fn trusted_proxies_rejects_garbage() {
        let err =
            Config::from_source(source(&[("SERVER_TRUSTED_PROXIES", "not-an-ip")])).unwrap_err();
        assert_eq!(err, ConfigError::InvalidCidr("not-an-ip".to_string()));
        // Out-of-range prefix is rejected too.
        let err =
            Config::from_source(source(&[("SERVER_TRUSTED_PROXIES", "10.0.0.0/33")])).unwrap_err();
        assert_eq!(err, ConfigError::InvalidCidr("10.0.0.0/33".to_string()));
    }
}
