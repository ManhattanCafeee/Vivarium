//! Single-use rotating refresh tokens with hash-only storage.
//!
//! A refresh token is an opaque id minted from the operating system CSPRNG,
//! stored server-side alongside its user and expiry. Rotation is single-use:
//! [`RefreshTokenManager::rotate`] deletes the received token and reissues a
//! fresh pair, and a replayed (or concurrently raced) token fails because the
//! delete affected zero rows. No transactions or locks are needed — the delete
//! itself is the atomicity boundary, matching the reference implementation.
//!
//! Two properties shape the API:
//!
//! - **The store only sees digests.** The client holds the raw token; every
//!   [`RefreshTokenStore`] call receives its SHA-256 digest, so a leaked
//!   `refresh_tokens` table hands out no usable credential.
//! - **The stored user wins.** [`RefreshTokenManager::rotate`] hands the
//!   callback only the user id it read from the store and then overwrites
//!   `sub`, `exp` and `iat` on the claims it returns
//!   ([`AccessClaims::set_subject`]). A callback can therefore neither mint a
//!   token for someone else nor choose its lifetime. The access TTL is the
//!   manager's, applied on both `issue` and `rotate`.
//!
//! The table is app-owned; the recommended shape is
//! `refresh_tokens(token_hash VARCHAR(64) UNIQUE, user_id, expires_at)` (adapt
//! time columns to your driver — UTC `DATETIME`/`TIMESTAMP` or epoch seconds).
//! Map storage failures to [`ApiError::internal`].
//!
//! ```no_run
//! use chrono::{DateTime, Utc};
//! use parking_lot::Mutex;
//! use std::{collections::HashMap, sync::Arc, time::Duration};
//! use vivarium_web::{AccessClaims, Claims, RefreshTokenManager, RefreshTokenRecord, RefreshTokenStore};
//!
//! #[derive(Clone, Default)]
//! struct TokenStore(Arc<Mutex<HashMap<String, RefreshTokenRecord<u64>>>>);
//!
//! impl RefreshTokenStore for TokenStore {
//!     type UserId = u64;
//!
//!     async fn insert(&self, token_hash: &str, user: u64, expires_at: DateTime<Utc>)
//!         -> Result<(), vivarium_web::ApiError> {
//!         self.0.lock().insert(token_hash.to_string(), RefreshTokenRecord { user_id: user, expires_at });
//!         Ok(())
//!     }
//!     async fn lookup(&self, token_hash: &str)
//!         -> Result<Option<RefreshTokenRecord<u64>>, vivarium_web::ApiError> {
//!         Ok(self.0.lock().get(token_hash).cloned())
//!     }
//!     async fn remove(&self, token_hash: &str) -> Result<bool, vivarium_web::ApiError> {
//!         Ok(self.0.lock().remove(token_hash).is_some())
//!     }
//!     async fn remove_by_user(&self, user: u64) -> Result<u64, vivarium_web::ApiError> {
//!         let mut store = self.0.lock();
//!         let before = store.len() as u64;
//!         store.retain(|_, record| record.user_id != user);
//!         Ok(before - store.len() as u64)
//!     }
//! }
//!
//! # async fn example() -> Result<(), vivarium_web::ApiError> {
//! # use vivarium_web::Claims;
//! let manager = RefreshTokenManager::new(
//!     TokenStore::default(), "secret",
//!     Duration::from_secs(900), Duration::from_secs(30 * 24 * 3600),
//! );
//!
//! let mut claims = Claims {
//!     sub: 0, exp: 0, iat: 0, jti: None,
//!     scope: vec!["user:read".to_string()], extra: Default::default(),
//! };
//! let pair = manager.issue(&mut claims, 7).await?;
//! assert_eq!(claims.subject(), 7);
//!
//! // `rebuild` re-reads the user; the manager overwrites sub/exp/iat anyway.
//! let rotated = manager
//!     .rotate(&pair.refresh_token, |_user| async {
//!         Ok::<Claims<u64>, vivarium_web::ApiError>(Claims {
//!             sub: 0, exp: 0, iat: 0, jti: None,
//!             scope: Vec::new(), extra: Default::default(),
//!         })
//!     })
//!     .await?;
//! # let _ = rotated;
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::jwt;
use crate::secrets::{generate_token, hash_token};
use crate::texts::texts;

/// A stored refresh token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshTokenRecord<U = i64> {
    /// The user this token was issued to.
    pub user_id: U,
    /// The instant this token expires.
    pub expires_at: DateTime<Utc>,
}

/// Storage back-end for refresh tokens.
///
/// Implement against your own schema (see the module docs). Every `token_hash`
/// this trait receives is a SHA-256 digest, never the value the client holds —
/// store and compare it as-is. `remove` returns `true` only when exactly one
/// row was deleted: that count is the single-use guarantee of rotation, so it
/// must come from the database (`rows_affected`), not from a prior read.
pub trait RefreshTokenStore: Send + Sync + 'static {
    /// The application's user id type.
    type UserId: Copy + Eq + Send + Sync + 'static;

    /// Persists a new token digest.
    fn insert(
        &self,
        token_hash: &str,
        user: Self::UserId,
        expires_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;

    /// Returns the stored record for this digest, if it exists.
    fn lookup(
        &self,
        token_hash: &str,
    ) -> impl Future<Output = Result<Option<RefreshTokenRecord<Self::UserId>>, ApiError>> + Send;

    /// Deletes exactly one token row; returns whether one was deleted.
    fn remove(&self, token_hash: &str) -> impl Future<Output = Result<bool, ApiError>> + Send;

    /// Deletes every token of a user (logout / password change), returning how
    /// many were deleted.
    fn remove_by_user(
        &self,
        user: Self::UserId,
    ) -> impl Future<Output = Result<u64, ApiError>> + Send;
}

/// The claims a [`RefreshTokenManager`] can fill in itself.
///
/// Implement it for the application's access-token claims so the manager can
/// stamp the identity and the lifetime it owns. The manager always calls
/// [`set_subject`](AccessClaims::set_subject) with the user id it read from the
/// store, so an implementation that ignores the call would let a callback mint
/// tokens for another user — honour it.
pub trait AccessClaims<U> {
    /// The subject these claims carry.
    fn subject(&self) -> U;

    /// Overwrites the subject. The manager passes the **stored** user id.
    fn set_subject(&mut self, sub: U);

    /// Overwrites the expiry (seconds since the Unix epoch).
    fn set_expiry(&mut self, exp: i64);

    /// Overwrites the issue time (seconds since the Unix epoch).
    fn set_issued_at(&mut self, iat: i64);
}

/// Ready-made access claims: the standard registered claims plus `scope` and
/// any extra fields an application adds.
///
/// `extra` is flattened into the JWT itself, so a token stays self-describing
/// without a bespoke struct; a bespoke struct is faster and type-safe when the
/// claims are fixed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claims<U = i64> {
    /// The subject: the user the token was issued to.
    pub sub: U,
    /// Expiry, in seconds since the Unix epoch.
    pub exp: i64,
    /// Issue time, in seconds since the Unix epoch.
    pub iat: i64,
    /// An optional token id, for revocation lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Permission codes carried by the token.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Any further claims, flattened into the token.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl<U: Clone> AccessClaims<U> for Claims<U> {
    fn subject(&self) -> U {
        self.sub.clone()
    }

    fn set_subject(&mut self, sub: U) {
        self.sub = sub;
    }

    fn set_expiry(&mut self, exp: i64) {
        self.exp = exp;
    }

    fn set_issued_at(&mut self, iat: i64) {
        self.iat = iat;
    }
}

/// Issues and rotates access + refresh token pairs.
#[derive(Clone)]
pub struct RefreshTokenManager<S> {
    store: S,
    secret: String,
    access_ttl: Duration,
    refresh_ttl: Duration,
}

impl<S: std::fmt::Debug> std::fmt::Debug for RefreshTokenManager<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshTokenManager")
            .field("store", &self.store)
            .field("secret", &"<redacted>")
            .field("access_ttl", &self.access_ttl)
            .field("refresh_ttl", &self.refresh_ttl)
            .finish()
    }
}

impl<S: RefreshTokenStore> RefreshTokenManager<S> {
    /// Constructs a manager with the given store, JWT secret and TTLs.
    ///
    /// `access_ttl` is stamped onto every access token this manager signs;
    /// `refresh_ttl` bounds the life of a refresh token.
    pub fn new(
        store: S,
        secret: impl Into<String>,
        access_ttl: Duration,
        refresh_ttl: Duration,
    ) -> Self {
        Self {
            store,
            secret: secret.into(),
            access_ttl,
            refresh_ttl,
        }
    }

    /// Accessor for the wrapped store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The access-token TTL the manager stamps onto every token.
    pub fn access_ttl(&self) -> Duration {
        self.access_ttl
    }

    /// The refresh-token TTL.
    pub fn refresh_ttl(&self) -> Duration {
        self.refresh_ttl
    }

    /// Issues a fresh pair: an access token carrying `claims` and a new
    /// single-use refresh token bound to `user`.
    ///
    /// `sub`, `exp` and `iat` are overwritten with `user` and the configured
    /// [`access_ttl`](Self::access_ttl) — the caller's other claims (scope, jti,
    /// extra fields) are signed as they are.
    pub async fn issue<C>(&self, claims: &mut C, user: S::UserId) -> Result<TokenPair, ApiError>
    where
        C: AccessClaims<S::UserId> + Serialize,
    {
        self.issue_pair(claims, user).await
    }

    /// Rotates: consumes the received refresh token and issues a fresh pair.
    ///
    /// `rebuild` re-reads the user identified by the **stored** record (that id
    /// is all it receives, so it cannot be steered by the caller's claims) and
    /// returns the claims for the new access token; the manager overwrites
    /// `sub`/`exp`/`iat` on them.
    ///
    /// A missing, expired, replayed, or concurrently raced token yields a 401
    /// with the catalog's
    /// [`invalid_refresh`](crate::texts::Texts::invalid_refresh) message. The
    /// token is consumed first and checked after, so an *expired* token is
    /// consumed too — keep that order in mind when adding rate limiting.
    pub async fn rotate<C, F, Fut>(&self, received: &str, rebuild: F) -> Result<TokenPair, ApiError>
    where
        C: AccessClaims<S::UserId> + Serialize,
        F: FnOnce(S::UserId) -> Fut,
        Fut: Future<Output = Result<C, ApiError>>,
    {
        let digest = hash_token(received);
        let stored = self
            .store
            .lookup(&digest)
            .await?
            .ok_or_else(invalid_refresh)?;

        // Delete first: whoever loses the race (or replays) deletes nothing and
        // is rejected, which is what makes rotation single-use.
        if !self.store.remove(&digest).await? {
            return Err(invalid_refresh());
        }
        if stored.expires_at <= Utc::now() {
            return Err(invalid_refresh());
        }

        let mut claims = rebuild(stored.user_id).await?;
        self.issue_pair(&mut claims, stored.user_id).await
    }

    /// Revokes every refresh token of a user (logout everywhere), returning
    /// how many were deleted.
    pub async fn revoke_all(&self, user: S::UserId) -> Result<u64, ApiError> {
        self.store.remove_by_user(user).await
    }

    /// Signs an access token for `user` and stores a fresh refresh token.
    async fn issue_pair<C>(&self, claims: &mut C, user: S::UserId) -> Result<TokenPair, ApiError>
    where
        C: AccessClaims<S::UserId> + Serialize,
    {
        let now = jsonwebtoken::get_current_timestamp() as i64;
        claims.set_subject(user);
        claims.set_issued_at(now);
        claims
            .set_expiry(now.saturating_add(self.access_ttl.as_secs().min(i64::MAX as u64) as i64));
        let access_token = jwt::sign_token(claims, &self.secret)?;

        let refresh_token = generate_token();
        self.store
            .insert(
                &hash_token(&refresh_token),
                user,
                expiry_after(self.refresh_ttl),
            )
            .await?;

        Ok(TokenPair {
            access_token,
            refresh_token,
            expires_in: self.access_ttl.as_secs(),
        })
    }
}

/// A freshly issued access + refresh pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPair {
    /// The HS256 access token, signed with the manager's secret.
    pub access_token: String,
    /// The single-use refresh token; only its digest is stored.
    pub refresh_token: String,
    /// The access token's lifetime in seconds, for the client's `expires_in`.
    pub expires_in: u64,
}

/// The 401 every rejected refresh token produces.
fn invalid_refresh() -> ApiError {
    ApiError::unauthorized(texts().invalid_refresh.clone())
}

/// `now + ttl`, saturating instead of overflowing for an absurd TTL (the token
/// then simply never expires).
fn expiry_after(ttl: Duration) -> DateTime<Utc> {
    let delta = TimeDelta::from_std(ttl).unwrap_or(TimeDelta::MAX);
    Utc::now()
        .checked_add_signed(delta)
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::decode_token;
    use parking_lot::Mutex;
    use std::sync::Arc;

    const SECRET: &str = "test-secret";
    const ACCESS_TTL: Duration = Duration::from_secs(900);
    const REFRESH_TTL: Duration = Duration::from_secs(86_400);

    type Store = Arc<Mutex<std::collections::HashMap<String, RefreshTokenRecord<u64>>>>;

    #[derive(Clone, Default)]
    struct InMemoryTokenStore(Store);

    impl InMemoryTokenStore {
        fn insert_record(&self, digest: &str, record: RefreshTokenRecord<u64>) {
            self.0.lock().insert(digest.to_string(), record);
        }

        fn contains(&self, digest: &str) -> bool {
            self.0.lock().contains_key(digest)
        }

        fn keys(&self) -> Vec<String> {
            self.0.lock().keys().cloned().collect()
        }
    }

    impl RefreshTokenStore for InMemoryTokenStore {
        type UserId = u64;

        async fn insert(
            &self,
            token_hash: &str,
            user: u64,
            expires_at: DateTime<Utc>,
        ) -> Result<(), ApiError> {
            self.insert_record(
                token_hash,
                RefreshTokenRecord {
                    user_id: user,
                    expires_at,
                },
            );
            Ok(())
        }

        async fn lookup(
            &self,
            token_hash: &str,
        ) -> Result<Option<RefreshTokenRecord<u64>>, ApiError> {
            Ok(self.0.lock().get(token_hash).cloned())
        }

        async fn remove(&self, token_hash: &str) -> Result<bool, ApiError> {
            Ok(self.0.lock().remove(token_hash).is_some())
        }

        async fn remove_by_user(&self, user: u64) -> Result<u64, ApiError> {
            let mut store = self.0.lock();
            let before = store.len() as u64;
            store.retain(|_, record| record.user_id != user);
            Ok(before - store.len() as u64)
        }
    }

    fn manager(store: InMemoryTokenStore) -> RefreshTokenManager<InMemoryTokenStore> {
        RefreshTokenManager::new(store, SECRET, ACCESS_TTL, REFRESH_TTL)
    }

    fn claims(sub: u64) -> Claims<u64> {
        Claims {
            sub,
            exp: 0,
            iat: 0,
            jti: None,
            scope: vec!["user:read".to_string()],
            extra: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn issue_stamps_the_user_and_the_ttl() {
        let store = InMemoryTokenStore::default();
        let manager = manager(store.clone());
        // A claims set that lies about the subject and the lifetime: the
        // manager owns both.
        let mut stale = claims(999);
        stale.exp = 1;

        let before = jsonwebtoken::get_current_timestamp() as i64;
        let pair = manager.issue(&mut stale, 7).await.expect("issue");
        let after = jsonwebtoken::get_current_timestamp() as i64;

        assert_eq!(stale.subject(), 7, "the claims are stamped in place");
        assert_eq!(pair.expires_in, ACCESS_TTL.as_secs());

        let decoded: Claims<u64> = decode_token(&pair.access_token, SECRET).expect("decodes");
        assert_eq!(decoded.sub, 7);
        assert!(
            (before..=after).contains(&decoded.iat),
            "iat {} outside [{before}, {after}]",
            decoded.iat
        );
        assert_eq!(decoded.exp, decoded.iat + ACCESS_TTL.as_secs() as i64);
        assert_eq!(decoded.scope, vec!["user:read".to_string()]);
    }

    #[tokio::test]
    async fn store_only_ever_sees_digests() {
        let store = InMemoryTokenStore::default();
        let manager = manager(store.clone());

        let pair = manager.issue(&mut claims(0), 7).await.expect("issue");
        assert_eq!(store.keys(), vec![hash_token(&pair.refresh_token)]);
        assert!(
            !store.contains(&pair.refresh_token),
            "the raw refresh token must never reach the store"
        );
        assert_eq!(pair.refresh_token.len(), 43, "base64url of 32 bytes");
    }

    #[tokio::test]
    async fn rotate_rebuilds_claims_from_the_stored_user() {
        let store = InMemoryTokenStore::default();
        let manager = manager(store.clone());
        let pair = manager.issue(&mut claims(0), 7).await.expect("issue");

        let mut seen_user = None;
        let rotated = manager
            .rotate(&pair.refresh_token, |user| {
                seen_user = Some(user);
                // The callback lies about the subject; the manager overrides it.
                async { Ok(claims(999)) }
            })
            .await
            .expect("rotate");

        assert_eq!(seen_user, Some(7), "the callback sees the stored user");
        let decoded: Claims<u64> = decode_token(&rotated.access_token, SECRET).expect("decodes");
        assert_eq!(decoded.sub, 7, "the stored user wins over the callback");
        assert_ne!(rotated.refresh_token, pair.refresh_token);
        assert!(!store.contains(&hash_token(&pair.refresh_token)));
        assert!(store.contains(&hash_token(&rotated.refresh_token)));
    }

    #[tokio::test]
    async fn rotation_is_single_use() {
        let store = InMemoryTokenStore::default();
        let manager = manager(store.clone());
        let pair = manager.issue(&mut claims(0), 7).await.expect("issue");

        manager
            .rotate(&pair.refresh_token, |_| async { Ok(claims(0)) })
            .await
            .expect("first rotation");

        let replay = manager
            .rotate(&pair.refresh_token, |_| async { Ok(claims(0)) })
            .await
            .expect_err("replay must fail");
        assert_eq!(replay.kind(), crate::error::ErrorKind::Unauthorized);
        assert_eq!(replay.message(), texts().invalid_refresh.as_ref());
    }

    #[tokio::test]
    async fn unknown_token_is_rejected_with_the_catalog_message() {
        let manager = manager(InMemoryTokenStore::default());
        let error = manager
            .rotate("nope", |_| async { Ok(claims(0)) })
            .await
            .expect_err("unknown token");
        assert_eq!(error.kind(), crate::error::ErrorKind::Unauthorized);
        assert_eq!(error.message(), texts().invalid_refresh.as_ref());
    }

    #[tokio::test]
    async fn expired_tokens_are_consumed_and_rejected() {
        let now = Utc::now();
        let store = InMemoryTokenStore::default();
        store.insert_record(
            &hash_token("expired"),
            RefreshTokenRecord {
                user_id: 7,
                expires_at: now - TimeDelta::seconds(1),
            },
        );
        let manager = manager(store.clone());

        let error = manager
            .rotate("expired", |_| async { Ok(claims(0)) })
            .await
            .expect_err("expired token");
        assert_eq!(error.kind(), crate::error::ErrorKind::Unauthorized);
        assert!(
            !store.contains(&hash_token("expired")),
            "an expired token is consumed as well"
        );
    }

    #[tokio::test]
    async fn revoke_all_removes_every_token_of_a_user() {
        let store = InMemoryTokenStore::default();
        let manager = manager(store.clone());
        manager.issue(&mut claims(0), 7).await.expect("issue");
        manager.issue(&mut claims(0), 7).await.expect("issue");
        manager.issue(&mut claims(0), 8).await.expect("issue");

        assert_eq!(manager.revoke_all(7).await.expect("revoke"), 2);
        assert_eq!(store.keys().len(), 1);
    }
}
