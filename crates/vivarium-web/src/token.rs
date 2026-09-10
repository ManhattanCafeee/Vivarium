//! Single-use rotating refresh tokens.
//!
//! A refresh token is an opaque, randomly generated id stored server-side
//! alongside its user and expiry. Rotation is single-use: [`RefreshTokenManager::rotate`]
//! deletes the received token and reissues a fresh pair, and a replayed (or
//! concurrently raced) token fails because the delete affected zero rows. No
//! transactions or locks are needed — the delete itself is the atomicity
//! boundary, matching the reference implementation.
//!
//! The table is app-owned; the recommended shape is
//! `refresh_tokens(token UNIQUE, user_id, expires_at)` (adapt time columns to
//! your driver — epoch seconds is portable). Map storage failures to
//! [`ApiError::internal`].
//!
//! ```no_run
//! use std::{collections::HashMap, sync::Arc, time::Duration};
//! use parking_lot::Mutex;
//! use serde::{Deserialize, Serialize};
//! use vivarium_web::{RefreshTokenManager, RefreshTokenRecord, RefreshTokenStore};
//!
//! #[derive(Clone, Default)]
//! struct TokenStore(Arc<Mutex<HashMap<String, RefreshTokenRecord>>>);
//!
//! impl RefreshTokenStore for TokenStore {
//!     async fn insert(&self, token: &str, user_id: i64, expires_at: std::time::SystemTime)
//!         -> Result<(), vivarium_web::ApiError> {
//!         self.0.lock().insert(token.to_string(), RefreshTokenRecord { user_id, expires_at });
//!         Ok(())
//!     }
//!     async fn lookup(&self, token: &str)
//!         -> Result<Option<RefreshTokenRecord>, vivarium_web::ApiError> {
//!         Ok(self.0.lock().get(token).cloned())
//!     }
//!     async fn remove(&self, token: &str) -> Result<bool, vivarium_web::ApiError> {
//!         Ok(self.0.lock().remove(token).is_some())
//!     }
//!     async fn remove_by_user(&self, user_id: i64) -> Result<(), vivarium_web::ApiError> {
//!         self.0.lock().retain(|_, r| r.user_id != user_id);
//!         Ok(())
//!     }
//! }
//!
//! #[derive(Clone, Serialize, Deserialize)]
//! struct Claims { sub: i64, exp: u64 }
//!
//! # async fn example() -> Result<(), vivarium_web::ApiError> {
//! let manager = RefreshTokenManager::new(
//!     TokenStore::default(), "secret",
//!     Duration::from_secs(900), Duration::from_secs(30 * 24 * 3600),
//! );
//! let pair = manager.issue(&Claims { sub: 7, exp: 0 }, 7).await?;
//! let rotated = manager.rotate(&pair.refresh_token, &Claims { sub: 7, exp: 0 }).await?;
//! # Ok(())
//! # }
//! ```

use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::error::ApiError;
use crate::jwt;
use uuid::Uuid;

/// A stored refresh token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshTokenRecord {
    /// The user this token was issued to.
    pub user_id: i64,
    /// The instant this token expires.
    pub expires_at: SystemTime,
}

/// Storage back-end for refresh tokens.
///
/// Implement against your own schema (see module docs). `remove` returns
/// `true` only when exactly one row was deleted: the single-use guarantee of
/// rotation, enforced by the SQL `rows_affected` count.
pub trait RefreshTokenStore: Send + Sync + 'static {
    /// Persists a new token.
    fn insert<'a>(
        &'a self,
        token: &'a str,
        user_id: i64,
        expires_at: SystemTime,
    ) -> impl Future<Output = Result<(), ApiError>> + Send + 'a;

    /// Returns the stored record for this token, if it exists.
    fn lookup<'a>(
        &'a self,
        token: &'a str,
    ) -> impl Future<Output = Result<Option<RefreshTokenRecord>, ApiError>> + Send + 'a;

    /// Deletes exactly one token row; returns whether one was deleted.
    fn remove<'a>(
        &'a self,
        token: &'a str,
    ) -> impl Future<Output = Result<bool, ApiError>> + Send + 'a;

    /// Deletes every token of a user (logout / password change).
    fn remove_by_user(
        &self,
        user_id: i64,
    ) -> impl Future<Output = Result<(), ApiError>> + Send + '_;
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

    /// The access-token TTL.
    pub fn access_ttl(&self) -> Duration {
        self.access_ttl
    }

    /// The refresh-token TTL.
    pub fn refresh_ttl(&self) -> Duration {
        self.refresh_ttl
    }

    /// Issues a fresh pair: an access token carrying `claims` and a new
    /// single-use refresh token bound to `user_id`.
    ///
    /// `claims` is signed as-is; include the expiry arithmetic yourself or
    /// use the TTLs via [`access_ttl`](RefreshTokenManager::access_ttl).
    pub async fn issue<C: Serialize>(
        &self,
        claims: &C,
        user_id: i64,
    ) -> Result<TokenPair, ApiError> {
        let access_token = jwt::sign_token(claims, &self.secret)?;
        let refresh_token = Uuid::new_v4().to_string();
        self.store
            .insert(&refresh_token, user_id, now_plus(self.refresh_ttl))
            .await?;
        Ok(TokenPair {
            access_token,
            refresh_token,
        })
    }

    /// Rotates: consumes the received refresh token (single-use) and issues a
    /// fresh pair.
    ///
    /// Call with a freshly rebuilt `claims` set (e.g. after re-loading the
    /// user) — the new access token carries it verbatim. A missing, expired,
    /// replayed, or concurrently raced token yields
    /// [`ApiError::unauthorized`] with "invalid or expired refresh token".
    pub async fn rotate<C: Serialize>(
        &self,
        received: &str,
        claims: &C,
    ) -> Result<TokenPair, ApiError> {
        let stored = self
            .store
            .lookup(received)
            .await?
            .ok_or_else(invalid_refresh)?;

        let removed = self.store.remove(received).await?;
        if !removed {
            // Concurrent replay: the other request already consumed it.
            return Err(invalid_refresh());
        }
        if stored.expires_at <= SystemTime::now() {
            return Err(invalid_refresh());
        }

        let access_token = jwt::sign_token(claims, &self.secret)?;
        let refresh_token = Uuid::new_v4().to_string();
        self.store
            .insert(&refresh_token, stored.user_id, now_plus(self.refresh_ttl))
            .await?;
        Ok(TokenPair {
            access_token,
            refresh_token,
        })
    }

    /// Revokes every refresh token of a user (logout everywhere).
    pub async fn revoke_all(&self, user_id: i64) -> Result<(), ApiError> {
        self.store.remove_by_user(user_id).await
    }
}

/// A freshly issued access + refresh pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPair {
    /// The HS256 access token, signed with the manager's secret.
    pub access_token: String,
    /// The single-use refresh token.
    pub refresh_token: String,
}

/// The 401 every rejected refresh token produces.
///
/// The text is fixed here rather than taken from [`Texts`](crate::texts::Texts):
/// the token layer's messages move into the catalog when the refresh-token
/// redesign lands, and until then a replayed or expired token keeps this exact
/// wording (clients distinguish it from a missing session).
fn invalid_refresh() -> ApiError {
    ApiError::unauthorized("invalid or expired refresh token")
}

/// `now + ttl`, falling back to `now` on overflow so an absurdly large TTL
/// yields an immediately-expired token instead of panicking.
fn now_plus(ttl: Duration) -> SystemTime {
    let now = SystemTime::now();
    now.checked_add(ttl).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde::Deserialize;
    use std::collections::HashMap;
    use std::sync::Arc;

    const SECRET: &str = "test-secret";

    #[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
    struct Claims {
        sub: String,
        exp: u64,
    }

    fn claims(sub: &str) -> Claims {
        Claims {
            sub: sub.to_string(),
            exp: jsonwebtoken::get_current_timestamp() + 3600,
        }
    }

    #[derive(Clone, Default, Debug)]
    struct InMemoryTokenStore(Arc<Mutex<HashMap<String, RefreshTokenRecord>>>);

    impl RefreshTokenStore for InMemoryTokenStore {
        async fn insert(
            &self,
            token: &str,
            user_id: i64,
            expires_at: SystemTime,
        ) -> Result<(), ApiError> {
            self.0.lock().insert(
                token.to_string(),
                RefreshTokenRecord {
                    user_id,
                    expires_at,
                },
            );
            Ok(())
        }

        async fn lookup(&self, token: &str) -> Result<Option<RefreshTokenRecord>, ApiError> {
            Ok(self.0.lock().get(token).cloned())
        }

        async fn remove(&self, token: &str) -> Result<bool, ApiError> {
            Ok(self.0.lock().remove(token).is_some())
        }

        async fn remove_by_user(&self, user_id: i64) -> Result<(), ApiError> {
            self.0.lock().retain(|_, r| r.user_id != user_id);
            Ok(())
        }
    }

    fn manager(store: InMemoryTokenStore) -> RefreshTokenManager<InMemoryTokenStore> {
        RefreshTokenManager::new(
            store,
            SECRET,
            Duration::from_secs(60),
            Duration::from_secs(3600),
        )
    }

    #[tokio::test]
    async fn issue_signs_access_and_stores_refresh() {
        let store = InMemoryTokenStore::default();
        let mgr = manager(store);
        let claims = claims("alice");

        let pair = mgr.issue(&claims, 7).await.expect("issue");
        assert!(!pair.access_token.is_empty());
        assert!(!pair.refresh_token.is_empty());
        let decoded: Claims = jwt::decode_token(&pair.access_token, SECRET).expect("decode");
        assert_eq!(decoded, claims);
    }

    #[tokio::test]
    async fn rotation_consumes_old_and_issues_new_pair() {
        let store = InMemoryTokenStore::default();
        let mgr = manager(store.clone());
        let claims = claims("alice");

        let pair = mgr.issue(&claims, 7).await.expect("issue");
        let rotated = mgr
            .rotate(&pair.refresh_token, &claims)
            .await
            .expect("rotate");
        assert_ne!(rotated.refresh_token, pair.refresh_token);
        let decoded: Claims = jwt::decode_token(&rotated.access_token, SECRET).expect("decode");
        assert_eq!(decoded, claims);

        // Replay of the consumed token must fail.
        let err = mgr
            .rotate(&pair.refresh_token, &claims)
            .await
            .expect_err("replay must fail");
        assert_eq!(err.kind(), crate::error::ErrorKind::Unauthorized);
    }

    #[tokio::test]
    async fn unknown_token_is_rejected() {
        let mgr = manager(InMemoryTokenStore::default());
        let err = mgr
            .rotate("no-such-token", &claims("alice"))
            .await
            .expect_err("unknown token must fail");
        assert_eq!(err.kind(), crate::error::ErrorKind::Unauthorized);
        assert_eq!(err.to_string(), "invalid or expired refresh token");
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let store = InMemoryTokenStore::default();
        store
            .insert("expired", 7, SystemTime::now() - Duration::from_secs(10))
            .await
            .expect("insert");
        let mgr = manager(store);

        let err = mgr
            .rotate("expired", &claims("alice"))
            .await
            .expect_err("expired token must fail");
        assert_eq!(err.kind(), crate::error::ErrorKind::Unauthorized);
    }

    #[tokio::test]
    async fn revoke_all_blocks_rotation() {
        let store = InMemoryTokenStore::default();
        let mgr = manager(store.clone());
        let pair = mgr.issue(&claims("alice"), 7).await.expect("issue");

        mgr.revoke_all(7).await.expect("revoke");
        assert!(store.0.lock().is_empty());
        let err = mgr
            .rotate(&pair.refresh_token, &claims("alice"))
            .await
            .expect_err("revoked token must fail");
        assert_eq!(err.kind(), crate::error::ErrorKind::Unauthorized);
    }

    #[test]
    fn debug_redacts_secret() {
        let mgr = manager(InMemoryTokenStore::default());
        let debug = format!("{mgr:?}");
        assert!(!debug.contains(SECRET), "secret leaked in Debug: {debug}");
        assert!(debug.contains("redacted"));
    }
}
