//! Resolving the server URL and API token from flags, environment, and the
//! optional `~/.config/sdt/config.toml` file.
//!
//! Precedence is strictly **flag > env > file**, with a built-in default for
//! the server. The file format is a tiny two-key TOML; a full TOML parser would
//! be overkill, so it is read and written by hand.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

/// Where the server lives when nothing else is configured.
pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";

/// Values read from the config file (either key may be absent).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileConfig {
    pub server: Option<String>,
    pub token: Option<String>,
}

/// Picks the first present source in precedence order: flag, then env, then
/// file. This is the whole of the precedence policy, kept pure so it can be
/// tested with injected values rather than real flags and environment.
pub fn pick(flag: Option<String>, env: Option<String>, file: Option<String>) -> Option<String> {
    flag.or(env).or(file)
}

/// `~/.config/sdt/config.toml`. Uses this literal path on every platform (not
/// the OS config dir) to match the documented location.
pub fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("sdt").join("config.toml"))
}

/// Loads the config file if present. A missing or unreadable file yields an
/// empty config rather than an error — it is only ever a fallback.
pub fn load_file() -> FileConfig {
    let Some(path) = config_path() else {
        return FileConfig::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_toml(&text),
        Err(_) => FileConfig::default(),
    }
}

/// Parses the `server = "…"` / `token = "…"` subset of TOML we write. Blank
/// lines and `#` comments are ignored; unknown keys are tolerated.
pub fn parse_toml(text: &str) -> FileConfig {
    let mut cfg = FileConfig::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "server" => cfg.server = Some(value),
            "token" => cfg.token = Some(value),
            _ => {}
        }
    }
    cfg
}

/// Renders the config file body.
pub fn serialize_config(server: &str, token: &str) -> String {
    format!(
        "# Stake Dev Tool CLI config — written by `sdt login --save`.\n\
         server = \"{}\"\n\
         token = \"{}\"\n",
        escape(server),
        escape(token),
    )
}

/// Writes `server`/`token` to the config file, creating parent directories.
/// On Unix the file is chmod 600 (it holds a secret); on Windows this is a
/// no-op, as there is no portable equivalent.
pub fn save(server: &str, token: &str) -> Result<PathBuf> {
    let path = config_path().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, serialize_config(server, token))
        .with_context(|| format!("writing {}", path.display()))?;
    restrict_permissions(&path)?;
    Ok(path)
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 600 {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

/// Strips one layer of matching single or double quotes.
fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Escapes the two characters that would break a double-quoted TOML string.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_is_flag_then_env_then_file() {
        let f = || Some("flag".to_string());
        let e = || Some("env".to_string());
        let c = || Some("file".to_string());

        assert_eq!(pick(f(), e(), c()).as_deref(), Some("flag"));
        assert_eq!(pick(None, e(), c()).as_deref(), Some("env"));
        assert_eq!(pick(None, None, c()).as_deref(), Some("file"));
        assert_eq!(pick(None, None, None), None);
    }

    #[test]
    fn parses_quoted_and_bare_values() {
        let cfg = parse_toml(
            "# comment\n\
             server = \"https://example.test\"\n\
             \n\
             token = sdt_pat_abc\n\
             unknown = 1\n",
        );
        assert_eq!(cfg.server.as_deref(), Some("https://example.test"));
        assert_eq!(cfg.token.as_deref(), Some("sdt_pat_abc"));
    }

    #[test]
    fn serialize_then_parse_roundtrips() {
        let body = serialize_config("https://api.test", "sdt_pat_xyz");
        let cfg = parse_toml(&body);
        assert_eq!(cfg.server.as_deref(), Some("https://api.test"));
        assert_eq!(cfg.token.as_deref(), Some("sdt_pat_xyz"));
    }

    #[test]
    fn empty_input_yields_empty_config() {
        assert_eq!(parse_toml(""), FileConfig::default());
    }
}
