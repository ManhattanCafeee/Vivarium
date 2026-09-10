//! Argon2 password hashing, timing-safe login verification, and transparent
//! parameter upgrades.
//!
//! [`hash`] produces PHC-format strings suitable for a `VARCHAR` column,
//! [`verify`] checks a plaintext password against one, and [`verify_login`] is
//! the lookup-aware wrapper: when no stored hash exists (unknown username), it
//! verifies against an internal dummy hash so the "unknown user" and "wrong
//! password" paths take the same Argon2 time, defeating username-enumeration
//! timing side channels — exactly while the stored rows use the parameters the
//! dummy is hashed with (see [`DUMMY_HASH`](self)).
//!
//! Password parameters age: hardware gets faster and recommendations move.
//! [`verify_and_upgrade`] does the whole login in one call — verify, then
//! re-hash with the parameters the application now wants, so the caller can
//! persist the upgrade while the plaintext is still at hand.
//!
//! ```no_run
//! use vivarium_web::password::{Argon2Params, VerifyOutcome, hash, verify_and_upgrade};
//!
//! let stored = hash("correct horse battery staple").expect("hash");
//! let params = Argon2Params::default();
//!
//! match verify_and_upgrade("correct horse battery staple", Some(&stored), params).expect("verify") {
//!     VerifyOutcome::Valid => {}                     // nothing to do
//!     VerifyOutcome::ValidNeedsRehash(upgraded) => { let _ = upgraded; /* persist */ }
//!     VerifyOutcome::Invalid => { /* reject the login */ }
//! }
//! ```

use std::error::Error as _;
use std::sync::LazyLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

use crate::error::ApiError;

/// The password hashed into [`DUMMY_HASH`]; its value is never a real password.
const DUMMY_PASSWORD: &str = "vivarium-timing-dummy";

/// The PHC algorithm identifier this crate hashes with.
const ALGORITHM_ID: &str = "argon2id";

/// The Argon2 version this crate hashes with, as written in a PHC string
/// (`v=19`, i.e. 0x13).
const VERSION: u32 = 19;

/// Cached dummy PHC string verified against when no stored hash exists.
///
/// Computed on first use; a failure to hash a constant cannot be cloned out of
/// a stored [`ApiError`], so the failure's detail is cached instead and turned
/// back into a fresh error on the (unreachable in practice) path that needs it.
///
/// It carries the current [`Argon2Params::default`] cost, so the dummy verify
/// costs exactly what a row hashed with the recommended profile costs. A store
/// whose rows still use weaker parameters makes the unknown-user path *more*
/// expensive than its wrong-password path, so login latency can still separate
/// "no such user" from "wrong password" for those rows — use
/// [`verify_and_upgrade`] to retire them.
static DUMMY_HASH: LazyLock<Result<String, String>> = LazyLock::new(|| {
    hash(DUMMY_PASSWORD).map_err(|error| {
        error
            .source()
            .map(ToString::to_string)
            .unwrap_or_else(|| "password hashing failed".to_string())
    })
});

/// The Argon2id cost parameters of a password column.
///
/// [`Default`] is the library's recommendation — Argon2id v19 with
/// `m = 19456` KiB, `t = 2`, `p = 1`, the OWASP-recommended profile and the
/// parameters the reference implementation hashes with, so existing hashes keep
/// verifying unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argon2Params {
    /// Memory cost, in KiB.
    pub m_cost: u32,
    /// Time cost: how many passes over that memory.
    pub t_cost: u32,
    /// Parallelism: how many lanes.
    pub p_cost: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            m_cost: Params::DEFAULT_M_COST,
            t_cost: Params::DEFAULT_T_COST,
            p_cost: Params::DEFAULT_P_COST,
        }
    }
}

impl Argon2Params {
    /// The Argon2 hasher these parameters describe.
    fn hasher(self) -> Result<Argon2<'static>, ApiError> {
        // `argon2::Error` is `no_std` and carries no `Error` impl; a PHC error
        // describes the same failure and does.
        let params = Params::new(self.m_cost, self.t_cost, self.p_cost, None)
            .map_err(argon2::password_hash::Error::from)
            .map_err(ApiError::internal)?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }
}

/// The outcome of [`verify_and_upgrade`].
#[derive(Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// The password is correct and the stored hash already uses the wanted
    /// parameters.
    Valid,
    /// The password is correct, but the stored hash is weaker than the wanted
    /// parameters. The payload is a fresh hash of the same password to persist
    /// (and only to persist — it is a secret).
    ValidNeedsRehash(String),
    /// The password is wrong, the user does not exist, or the login must be
    /// rejected.
    Invalid,
}

impl std::fmt::Debug for VerifyOutcome {
    /// Formats the variant name only: the rehash payload is a credential and
    /// must not reach a log through a `{:?}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Valid => f.write_str("Valid"),
            Self::ValidNeedsRehash(_) => f.write_str("ValidNeedsRehash(<redacted>)"),
            Self::Invalid => f.write_str("Invalid"),
        }
    }
}

/// Hashes `password` with Argon2id and returns the PHC-format string.
///
/// The salt is freshly generated per call with a cryptographic RNG.
pub fn hash(password: &str) -> Result<String, ApiError> {
    hash_with(password, Argon2Params::default())
}

/// Hashes `password` with the given parameters, returning a PHC string.
///
/// The parameters travel inside the PHC string, so a hash verifies with
/// [`verify`] no matter which parameters produced it.
pub fn hash_with(password: &str, params: Argon2Params) -> Result<String, ApiError> {
    let salt = SaltString::generate(OsRng);
    params
        .hasher()?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(ApiError::internal)
}

/// Verifies `password` against `stored_hash` (a PHC string from [`hash`]).
///
/// Returns `Ok(false)` only for a wrong password. A `stored_hash` that cannot
/// be parsed *or* whose parameters argon2 refuses is `Err(ApiError::internal)`
/// — that is corruption, not a failed login, and it must not masquerade as one
/// (the user would be locked out with no trace). The detail stays in the error
/// source for the logs.
pub fn verify(password: &str, stored_hash: &str) -> Result<bool, ApiError> {
    let parsed = parse(stored_hash)?;
    match Argon2::default().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(err) => Err(ApiError::internal(err)),
    }
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
            Err(detail) => return Err(ApiError::internal(detail.clone())),
        },
    };
    verify(password, target)
}

/// Whether `stored_hash` is weaker than `params`.
///
/// Only a stored hash that is weaker in some cost parameter is flagged, never
/// one that is stronger: rewriting a stronger hash with the application's
/// parameters would silently *lower* that credential's cost. The algorithm and
/// version are compared as well, since a row from an Argon2i or pre-v19 system
/// verifies correctly and would otherwise never be upgraded; the salt and the
/// output length do not age. A malformed hash is `Err(ApiError::internal)`,
/// like [`verify`].
pub fn needs_rehash(stored_hash: &str, params: Argon2Params) -> Result<bool, ApiError> {
    let parsed = parse(stored_hash)?;
    if parsed.algorithm.as_str() != ALGORITHM_ID || parsed.version != Some(VERSION) {
        return Ok(true);
    }
    let current = Params::try_from(&parsed).map_err(ApiError::internal)?;
    Ok(current.m_cost() < params.m_cost
        || current.t_cost() < params.t_cost
        || current.p_cost() < params.p_cost)
}

/// Verifies a login and, when the stored hash has aged, re-hashes it.
///
/// This is [`verify_login`] plus the upgrade: a correct password whose stored
/// hash is weaker than `params` comes back as
/// [`VerifyOutcome::ValidNeedsRehash`] carrying a fresh hash of that same
/// password, ready to persist. Nothing is written by this call — the storage
/// belongs to the application.
pub fn verify_and_upgrade(
    password: &str,
    stored_hash: Option<&str>,
    params: Argon2Params,
) -> Result<VerifyOutcome, ApiError> {
    let Some(stored_hash) = stored_hash else {
        // Unknown user: burn the same Argon2 work, then reject.
        let _ = verify_login(password, None)?;
        return Ok(VerifyOutcome::Invalid);
    };
    if !verify(password, stored_hash)? {
        return Ok(VerifyOutcome::Invalid);
    }
    if needs_rehash(stored_hash, params)? {
        return Ok(VerifyOutcome::ValidNeedsRehash(hash_with(
            password, params,
        )?));
    }
    Ok(VerifyOutcome::Valid)
}

/// Parses a PHC string, mapping corruption to an internal error.
fn parse(stored_hash: &str) -> Result<PasswordHash<'_>, ApiError> {
    PasswordHash::new(stored_hash).map_err(ApiError::internal)
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
    fn default_params_match_the_reference_profile() {
        let params = Argon2Params::default();
        assert_eq!(params.m_cost, 19 * 1024);
        assert_eq!(params.t_cost, 2);
        assert_eq!(params.p_cost, 1);

        // Existing hashes (Argon2id v19, those costs) need no upgrade.
        let stored = hash("correct horse battery staple").expect("hash succeeds");
        assert!(
            stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{stored}"
        );
        assert!(!needs_rehash(&stored, params).expect("inspects"));
    }

    #[test]
    fn weaker_hashes_are_flagged_for_rehash() {
        let weak = hash_with(
            "hunter2",
            Argon2Params {
                m_cost: 1024,
                t_cost: 1,
                p_cost: 1,
            },
        )
        .expect("hash succeeds");
        assert!(needs_rehash(&weak, Argon2Params::default()).expect("inspects"));

        // And the upgrade re-hashes with the wanted parameters.
        match verify_and_upgrade("hunter2", Some(&weak), Argon2Params::default()).expect("verify") {
            VerifyOutcome::ValidNeedsRehash(upgraded) => {
                assert!(
                    upgraded.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
                    "{upgraded}"
                );
                assert!(verify("hunter2", &upgraded).expect("verify succeeds"));
                assert!(!needs_rehash(&upgraded, Argon2Params::default()).expect("inspects"));
            }
            other => panic!("expected an upgrade, got {other:?}"),
        }
    }

    #[test]
    fn malformed_hashes_are_internal_and_do_not_echo_the_hash() {
        for corrupt in [
            "not-a-phc-string",
            "$argon2id$v=19$m=oops,t=2,p=1$c2FsdA$aGFzaA",
        ] {
            let error = verify("irrelevant", corrupt).expect_err("must reject");
            assert_eq!(error.kind(), crate::error::ErrorKind::Internal);
            assert!(
                !error.message().contains(corrupt),
                "the message must not carry the stored hash: {}",
                error.message()
            );
            assert!(needs_rehash(corrupt, Argon2Params::default()).is_err());
        }
    }

    #[test]
    fn verify_and_upgrade_rejects_wrong_passwords_and_unknown_users() {
        let stored = hash("s3cret!").expect("hash succeeds");
        let params = Argon2Params::default();

        assert_eq!(
            verify_and_upgrade("wrong", Some(&stored), params).expect("verify"),
            VerifyOutcome::Invalid
        );
        assert_eq!(
            verify_and_upgrade("anything", None, params).expect("verify"),
            VerifyOutcome::Invalid
        );
        assert_eq!(
            verify_and_upgrade("s3cret!", Some(&stored), params).expect("verify"),
            VerifyOutcome::Valid
        );
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
        assert_eq!(err.kind(), crate::error::ErrorKind::Internal);
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn stronger_hashes_are_left_alone() {
        let strong = hash_with(
            "hunter2",
            Argon2Params {
                m_cost: 65_536,
                t_cost: 4,
                p_cost: 2,
            },
        )
        .expect("hash succeeds");

        // A login with the application's weaker profile must not downgrade it.
        assert!(!needs_rehash(&strong, Argon2Params::default()).expect("inspects"));
        assert_eq!(
            verify_and_upgrade("hunter2", Some(&strong), Argon2Params::default()).expect("verify"),
            VerifyOutcome::Valid
        );
    }

    #[test]
    fn another_algorithm_or_version_is_flagged_for_rehash() {
        // Same shape and costs, but not the Argon2id/v19 profile this crate
        // hashes with: such a row verifies and would otherwise never upgrade.
        let hashed = hash("pw").expect("hash succeeds");

        let argon2i = hashed.replacen("$argon2id$", "$argon2i$", 1);
        assert!(needs_rehash(&argon2i, Argon2Params::default()).expect("inspects"));

        let old_version = hashed.replacen("$v=19$", "$v=16$", 1);
        assert!(needs_rehash(&old_version, Argon2Params::default()).expect("inspects"));
    }

    #[test]
    fn parameters_argon2_refuses_are_internal_not_a_wrong_password() {
        // Below argon2's minimum memory cost: the PHC string parses, so only
        // the parameter conversion can reject it. Reporting that as "wrong
        // password" would lock the user out with no trace.
        let rejected = hash("pw")
            .expect("hash succeeds")
            .replacen("m=19456", "m=4", 1);

        let err = verify("anything", &rejected).expect_err("must not be a wrong password");
        assert_eq!(err.kind(), crate::error::ErrorKind::Internal);
        assert!(needs_rehash(&rejected, Argon2Params::default()).is_err());
        assert_eq!(
            verify_and_upgrade("anything", Some(&rejected), Argon2Params::default())
                .expect_err("must not be a wrong password")
                .kind(),
            crate::error::ErrorKind::Internal
        );
    }

    #[test]
    fn verify_outcome_debug_redacts_the_new_hash() {
        let upgraded = hash("hunter2").expect("hash succeeds");
        let outcome = VerifyOutcome::ValidNeedsRehash(upgraded.clone());
        let printed = format!("{outcome:?}");

        assert!(!printed.contains(&upgraded), "{printed}");
        assert_eq!(printed, "ValidNeedsRehash(<redacted>)");
        assert_eq!(format!("{:?}", VerifyOutcome::Valid), "Valid");
    }
}
