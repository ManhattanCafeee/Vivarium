//! Argon2 password hashing and timing-safe login verification.
//!
//! [`hash`] produces PHC-format strings suitable for a `VARCHAR` column;
//! [`verify`] checks a plaintext password against one. [`verify_login`] is the
//! lookup-aware wrapper: when no stored hash exists (unknown username), it
//! verifies against an internal dummy hash so the "unknown user" and "wrong
//! password" paths take the same Argon2 time, defeating username-enumeration
//! timing side channels.

use std::sync::LazyLock;

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

use crate::error::ApiError;

/// The password hashed into [`DUMMY_HASH`]; its value is never a real password.
const DUMMY_PASSWORD: &str = "vivarium-timing-dummy";

/// Cached dummy PHC string verified against when no stored hash exists.
///
/// Computed on first use; a failure to hash a constant is treated as an
/// [`ApiError::Internal`] and surfaced, never panicked on.
static DUMMY_HASH: LazyLock<Result<String, ApiError>> = LazyLock::new(|| hash(DUMMY_PASSWORD));

/// Hashes `password` with Argon2 and returns the PHC-format string.
///
/// The salt is freshly generated per call with a cryptographic RNG.
pub fn hash(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|err| ApiError::Internal {
            system: format!("password hashing failed: {err}"),
        })
}

/// Verifies `password` against `stored_hash` (a PHC string from [`hash`]).
///
/// Returns `Ok(false)` for a mismatch; a malformed `stored_hash` is
/// `Err(ApiError::Internal)` — it indicates corruption, not a failed login.
pub fn verify(password: &str, stored_hash: &str) -> Result<bool, ApiError> {
    let parsed = PasswordHash::new(stored_hash).map_err(|err| ApiError::Internal {
        system: format!("invalid stored hash: {err}"),
    })?;
    let argon2 = Argon2::default();
    Ok(argon2.verify_password(password.as_bytes(), &parsed).is_ok())
}

/// Verifies a login attempt, keeping the "no such user" path constant-time
/// with the "wrong password" path.
///
/// Pass `stored_hash` when the user exists, `None` otherwise: with `None` the
/// password is verified against an internal dummy hash and `Ok(false)` is
/// returned. The dummy hash is computed once and cached.
pub fn verify_login(password: &str, stored_hash: Option<&str>) -> Result<bool, ApiError> {
    let target = match stored_hash {
        Some(hash) => hash,
        None => match &*DUMMY_HASH {
            Ok(hash) => hash.as_str(),
            Err(err) => return Err(err.clone()),
        },
    };
    verify(password, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let password = "correct horse battery staple";
        let stored = hash(password).expect("hash succeeds");
        assert!(verify(password, &stored).expect("verify succeeds"));
        assert!(!verify("wrong password", &stored).expect("verify succeeds"));
    }

    #[test]
    fn verify_login_unknown_user_returns_false() {
        assert!(!verify_login("anything", None).expect("dummy verify succeeds"));
    }

    #[test]
    fn verify_login_existing_user_checks_hash() {
        let password = "s3cret!";
        let stored = hash(password).expect("hash succeeds");
        assert!(verify_login(password, Some(&stored)).expect("verify succeeds"));
        assert!(!verify_login("wrong", Some(&stored)).expect("verify succeeds"));
    }

    #[test]
    fn verify_login_malformed_hash_is_internal() {
        let err = verify_login("x", Some("not-a-phc-string")).expect_err("must reject");
        assert!(matches!(err, ApiError::Internal { .. }));
    }
}
