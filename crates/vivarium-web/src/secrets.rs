//! Credential material: random ids and their storage digests.
//!
//! Session ids and refresh tokens are bearer credentials — whoever holds the
//! string is authenticated as its owner. The library therefore never hands the
//! raw value to a store: [`SessionAuth`](crate::session::SessionAuth) and
//! [`RefreshTokenManager`](crate::token::RefreshTokenManager) persist, look up
//! and delete by [`hash_token`] digest only, so a leaked session table (a dump,
//! a replica, a backup) hands out no live credentials.
//!
//! The digest is the lowercase hex of `SHA-256(token)`: 64 ASCII characters,
//! the width the recommended schema stores (`VARCHAR(64)`).
//!
//! ```
//! use vivarium_web::hash_token;
//!
//! // Deterministic, and stable across processes and releases.
//! assert_eq!(hash_token("sid"), hash_token("sid"));
//! assert_eq!(hash_token("sid").len(), 64);
//! ```

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Entropy per generated id: 32 bytes (256 bits), far beyond guessing.
const TOKEN_BYTES: usize = 32;

/// The SHA-256 digest of `token`, as lowercase hex.
///
/// This is what a store persists and looks up; the raw `token` is what the
/// client holds. Never log the raw value.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        // Writing into a `String` cannot fail.
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
    }
    hex
}

/// A fresh opaque id: 32 bytes from the operating system CSPRNG, base64url
/// without padding (43 characters, no character a cookie has to escape).
pub(crate) fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn hash_matches_the_sha256_test_vectors() {
        assert_eq!(
            hash_token(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn generated_ids_are_url_safe_and_unique() {
        let first = generate_token();
        let second = generate_token();

        assert_eq!(first.len(), 43, "32 bytes in base64url: {first}");
        assert_ne!(first, second);
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "not base64url: {first}"
        );
    }

    #[test]
    fn digests_are_unique_per_token() {
        let mut seen = HashSet::new();
        for index in 0..64 {
            assert!(seen.insert(hash_token(&format!("token-{index}"))));
        }
    }
}
