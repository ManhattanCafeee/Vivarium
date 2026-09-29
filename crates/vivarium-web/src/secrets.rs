//! Credential material and its checked shapes.
//!
//! [`Secret`] is an HS256 signing key, [`Ttl`] a non-zero lifetime and
//! [`Digest`] a credential's storage digest; each is built through a fallible
//! constructor, so an empty secret, a zero lifetime or a malformed digest
//! cannot reach the authentication surface.
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

use std::fmt;
use std::ops::Deref;
use std::time::Duration;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};

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

/// An HS256 signing secret.
///
/// Built only through [`Secret::try_new`], which rejects empty and
/// whitespace-only values: HMAC accepts a zero-length key, so a signer or
/// verifier handed one would produce and accept forged tokens.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Validates a secret: it must not be empty or whitespace-only.
    ///
    /// Strength (length, entropy) stays the deployment's responsibility;
    /// RFC 7518 §3.2 requires an HS256 key at least as long as the hash
    /// output (32 bytes).
    ///
    /// # Errors
    ///
    /// Returns [`SecretError`] for an empty or whitespace-only secret.
    pub fn try_new(secret: impl Into<String>) -> Result<Self, SecretError> {
        let secret = secret.into();
        if secret.trim().is_empty() {
            return Err(SecretError);
        }
        Ok(Self(secret))
    }

    /// The secret as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    /// Prints `<redacted>`: a secret must not reach a log through `{:?}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Deref for Secret {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Secret {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A non-zero lifetime: a session TTL or a refresh-token TTL.
///
/// Built only through [`Ttl::try_new`], so `Duration::ZERO` — which would
/// mint credentials that are born expired — cannot reach the auth surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ttl(Duration);

impl Ttl {
    /// Validates a lifetime: it must not be zero.
    ///
    /// # Errors
    ///
    /// Returns [`TtlError`] for [`Duration::ZERO`].
    pub fn try_new(ttl: Duration) -> Result<Self, TtlError> {
        if ttl.is_zero() {
            return Err(TtlError);
        }
        Ok(Self(ttl))
    }

    /// The lifetime as a [`Duration`].
    pub fn get(self) -> Duration {
        self.0
    }
}

/// A credential's SHA-256 digest: what storage receives and compares.
///
/// Built by [`Digest::of_token`] (hashing a raw credential) or parsed from a
/// stored column with [`Digest::parse_hex`]; those two are the only
/// constructors, so a store implementation cannot be handed the raw value
/// where it expects a digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Digest(String);

impl Digest {
    /// The digest of `token`, as 64 lowercase hex characters.
    pub fn of_token(token: &str) -> Self {
        Self(hash_token(token))
    }

    /// Parses a stored digest: exactly 64 lowercase hex characters.
    ///
    /// # Errors
    ///
    /// Returns [`DigestError`] when `hex` has the wrong length or contains a
    /// character outside `[0-9a-f]`.
    pub fn parse_hex(hex: &str) -> Result<Self, DigestError> {
        let valid = hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if valid {
            Ok(Self(hex.to_owned()))
        } else {
            Err(DigestError)
        }
    }

    /// The digest as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Digest {
    /// Prints the hex digest itself.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Deref for Digest {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Digest {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Returned by [`Secret::try_new`] for an empty or whitespace-only secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a secret must not be empty")]
pub struct SecretError;

/// Returned by [`Ttl::try_new`] for a zero lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a TTL must not be zero")]
pub struct TtlError;

/// Returned by [`Digest::parse_hex`] for a malformed stored digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a digest must be 64 lowercase hex characters")]
pub struct DigestError;

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

    #[test]
    fn try_new_rejects_empty_and_blank_secrets() {
        assert!(Secret::try_new("").is_err());
        assert!(Secret::try_new("   ").is_err());
        assert_eq!(Secret::try_new("x").expect("non-empty").as_str(), "x");
    }

    #[test]
    fn ttl_rejects_zero() {
        assert!(Ttl::try_new(Duration::ZERO).is_err());
        assert_eq!(
            Ttl::try_new(Duration::from_secs(1))
                .expect("non-zero")
                .get(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn digest_parse_rejects_the_wrong_shape() {
        assert!(Digest::parse_hex("").is_err());
        assert!(Digest::parse_hex(&"a".repeat(63)).is_err());
        assert!(Digest::parse_hex(&"A".repeat(64)).is_err());
        assert!(Digest::parse_hex(&"g".repeat(64)).is_err());
        assert_eq!(
            Digest::parse_hex(&hash_token("abc"))
                .expect("64 lowercase hex")
                .as_str(),
            hash_token("abc")
        );
    }

    #[test]
    fn digest_of_token_matches_hash_token() {
        let digest = Digest::of_token("sid");
        assert_eq!(digest.as_str(), hash_token("sid"));
        assert_eq!(digest.as_str().len(), 64);
    }
}
