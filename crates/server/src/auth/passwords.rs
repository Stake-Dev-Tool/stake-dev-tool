//! Argon2id password hashing via the `argon2` crate's recommended defaults.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

use crate::error::ApiError;

/// Hashes a password with Argon2id. The returned PHC string embeds the random
/// salt and the parameters, so verification needs nothing else.
pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| ApiError::internal(format!("password hashing failed: {e}")))
}

/// Verifies a password against a stored PHC hash. A mismatch and a malformed
/// stored hash both return `false` (never an error) so the caller gives one
/// uniform answer regardless of why verification failed.
pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_wrong_password() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("wrong password", &hash));
        // Argon2id output is salted: two hashes of the same input differ.
        let hash2 = hash_password("correct horse battery staple").unwrap();
        assert_ne!(hash, hash2);
    }

    #[test]
    fn malformed_hash_is_not_a_match() {
        assert!(!verify_password("anything", "not-a-phc-string"));
    }
}
