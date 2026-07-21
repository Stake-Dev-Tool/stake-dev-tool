//! Authentication plumbing: password hashing, sessions, API tokens, the device
//! pairing flow, GitHub OAuth, request extractors, and the login rate limiter.
//!
//! Uniform secret rule for every credential kind here: 32 bytes of OS entropy,
//! base64url-encoded with a self-identifying prefix, shown to the caller exactly
//! once. Only the sha256 of the full string is ever persisted, so lookups are by
//! hash and timing side-channels are a non-issue.

pub mod device;
pub mod extract;
pub mod github;
pub mod passwords;
pub mod ratelimit;
pub mod sessions;
pub mod tokens;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

/// Human-readable prefixes so a leaked secret is identifiable at a glance.
pub const SESSION_PREFIX: &str = "sdt_ses_";
pub const API_TOKEN_PREFIX: &str = "sdt_pat_";
pub const INVITE_PREFIX: &str = "sdt_inv_";
pub const DEVICE_PREFIX: &str = "sdt_dev_";

/// Generates `<prefix><base64url(32 random bytes)>`. 256 bits of entropy makes
/// the value unguessable; only its hash is stored.
pub fn generate_secret(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// sha256 of the full secret string — the 32-byte value stored in the `*_hash`
/// BYTEA columns and used for indexed, constant-cost lookups.
pub fn hash_secret(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

/// Unambiguous alphabet for the human-typed device `user_code`: no 0/O/1/I, and
/// exactly 32 symbols so `byte & 31` is a bias-free index.
const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generates an `XXXX-XXXX` device user code from the unambiguous alphabet.
pub fn generate_user_code() -> String {
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    let chars: String = bytes
        .iter()
        .map(|b| USER_CODE_ALPHABET[(*b as usize) & 31] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_carry_their_prefix_and_are_unique() {
        let a = generate_secret(API_TOKEN_PREFIX);
        let b = generate_secret(API_TOKEN_PREFIX);
        assert!(a.starts_with("sdt_pat_"));
        assert_ne!(a, b);
        // sha256 is 32 bytes and deterministic for a given input.
        assert_eq!(hash_secret(&a).len(), 32);
        assert_eq!(hash_secret(&a), hash_secret(&a));
        assert_ne!(hash_secret(&a), hash_secret(&b));
    }

    #[test]
    fn user_codes_are_formatted_and_unambiguous() {
        let code = generate_user_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.chars().nth(4), Some('-'));
        assert!(
            code.chars()
                .filter(|c| *c != '-')
                .all(|c| USER_CODE_ALPHABET.contains(&(c as u8)))
        );
    }
}
