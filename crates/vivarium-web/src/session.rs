//! Server-side cookie sessions with a sliding TTL and an absolute cap.
//!
//! A session is an opaque id minted from the operating system CSPRNG, carried
//! in a cookie and stored server-side. Two properties shape the API:
//!
//! - **The store only sees digests.** The cookie carries the raw id; every
//!   [`SessionStore`] call receives its SHA-256 [digest](SessionId::digest)
//!   instead. A leaked `sessions` table (a dump, a replica, a backup) therefore
//!   hands out no live sessions.
//! - **Two lifetimes.** `ttl` slides on activity; `absolute_ttl` caps the total
//!   life of a session — past that deadline the session dies regardless of
//!   activity, and renewal never extends beyond it.
//!
//! Layering, matching the reference implementation:
//! - [`session_layer`] resolves the cookie, validates the session against the
//!   store (deleting dead rows), stashes [`SessionCtx`] and [`SessionId`] in
//!   the request extensions and, after the handler, renews the session once it
//!   has been idle for half its TTL. The middleware never rejects a request —
//!   authentication decisions are the [`SessionCtx`] extractor's job.
//! - [`SessionAuth::start`] mints a session at login, [`SessionAuth::end`]
//!   deletes one at logout, and [`SessionAuth::set_cookie_value`] produces the
//!   `Set-Cookie` header value for the login response.
//!
//! `Secure`, `HttpOnly`, `SameSite=Lax` and `Path=/` are the defaults
//! ([`CookieOptions::new`]); local development over plain HTTP opts out
//! explicitly with [`CookieOptions::insecure`].
//!
//! ```no_run
//! use axum::{Router, routing::get};
//! use chrono::{DateTime, Utc};
//! use parking_lot::Mutex;
//! use std::{collections::HashMap, sync::Arc, time::Duration};
//! use vivarium_web::session::{
//!     CookieOptions, SessionAuth, SessionCtx, SessionRecord, SessionStore, session_layer,
//! };
//!
//! #[derive(Clone, Default)]
//! struct MyStore(Arc<Mutex<HashMap<String, SessionRecord<u64>>>>);
//!
//! impl SessionStore for MyStore {
//!     type UserId = u64;
//!
//!     async fn create(&self, id: &str, user: u64, created_at: DateTime<Utc>,
//!         expires_at: DateTime<Utc>, last_activity: DateTime<Utc>) -> Result<(), vivarium_web::ApiError> {
//!         self.0.lock().insert(id.to_string(), SessionRecord {
//!             user_id: user, created_at, expires_at, last_activity,
//!         });
//!         Ok(())
//!     }
//!     async fn find(&self, id: &str) -> Result<Option<SessionRecord<u64>>, vivarium_web::ApiError> {
//!         Ok(self.0.lock().get(id).cloned())
//!     }
//!     async fn touch(&self, id: &str, expires_at: DateTime<Utc>, last_activity: DateTime<Utc>)
//!         -> Result<(), vivarium_web::ApiError> {
//!         if let Some(record) = self.0.lock().get_mut(id) {
//!             record.expires_at = expires_at;
//!             record.last_activity = last_activity;
//!         }
//!         Ok(())
//!     }
//!     async fn remove(&self, id: &str) -> Result<bool, vivarium_web::ApiError> {
//!         Ok(self.0.lock().remove(id).is_some())
//!     }
//!     async fn remove_by_user(&self, user: u64) -> Result<u64, vivarium_web::ApiError> {
//!         let mut store = self.0.lock();
//!         let before = store.len() as u64;
//!         store.retain(|_, record| record.user_id != user);
//!         Ok(before - store.len() as u64)
//!     }
//! }
//!
//! async fn me(SessionCtx { user_id }: SessionCtx<u64>) -> String { user_id.to_string() }
//!
//! # async fn example() {
//! let auth = SessionAuth::new(
//!     MyStore::default(),
//!     CookieOptions::new("sid"),
//!     Duration::from_secs(3600),
//!     Some(Duration::from_secs(12 * 3600)),
//! );
//! let app: Router = Router::new().route("/me", get(me)).layer(session_layer(auth));
//! # }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::{self, FromFnLayer, Next};
use axum::response::Response;
use chrono::{DateTime, TimeDelta, Utc};
use cookie::Cookie;

pub use cookie::SameSite;

use crate::error::ApiError;
use crate::secrets::{generate_token, hash_token};
use crate::texts::texts;

/// The cookie carrying a session id.
///
/// The defaults are the safe ones — `Path=/`, `Secure`, `HttpOnly`,
/// `SameSite=Lax` — so a deployment only has to name the cookie. Fields are
/// public for the rest: set `domain` for a shared parent domain, or `path` to
/// confine the session to a subtree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieOptions {
    /// The cookie name (e.g. `sid`).
    pub name: String,
    /// The `Path` attribute.
    pub path: String,
    /// Whether the `Secure` attribute is set.
    pub secure: bool,
    /// Whether the `HttpOnly` attribute is set.
    pub http_only: bool,
    /// The `SameSite` attribute.
    pub same_site: SameSite,
    /// The `Domain` attribute, when the cookie is shared across subdomains.
    pub domain: Option<String>,
}

impl CookieOptions {
    /// Options for the cookie `name`, with the safe defaults.
    ///
    /// `path` is `/`, `secure` and `http_only` are on, and `same_site` is
    /// [`SameSite::Lax`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: "/".to_string(),
            secure: true,
            http_only: true,
            same_site: SameSite::Lax,
            domain: None,
        }
    }

    /// Drops `Secure`, for local development over plain HTTP.
    ///
    /// This is an explicit opt-out: a browser discards a `Secure` cookie sent
    /// over `http://`, so a development server needs this to be usable. Never
    /// ship it.
    #[must_use]
    pub fn insecure(mut self) -> Self {
        self.secure = false;
        self
    }
}

/// The id of a session, as it travels to the client.
///
/// 32 bytes from the operating system CSPRNG, base64url without padding. The
/// cookie carries this string; the store carries its
/// [`digest`](SessionId::digest) — the library hashes before every store call,
/// so the raw value never reaches storage or logs.
///
/// The extractor of the same name hands the authenticated session's id to a
/// handler, which is what logout needs:
///
/// ```
/// # use vivarium_web::session::SessionId;
/// let id = SessionId("8Qm…".to_string());
/// assert_eq!(id.digest().len(), 64);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct SessionId(pub String);

impl std::fmt::Debug for SessionId {
    /// Prints the digest, not the credential: an id is valid until its session
    /// expires, so a `{:?}` in a log or a span must not reveal it. The digest
    /// prefix still identifies the session well enough to correlate a log line
    /// with a store row.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let digest = self.digest();
        write!(f, "SessionId({}…)", &digest[..8])
    }
}

impl SessionId {
    /// The SHA-256 digest of this id, as stored and looked up.
    ///
    /// Calls [`hash_token`]; stores are expected to persist
    /// and query by this value, never by the raw id.
    pub fn digest(&self) -> String {
        hash_token(&self.0)
    }

    /// The raw id, as carried in the cookie.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Mints a fresh id from the operating system CSPRNG.
    fn generate() -> Self {
        Self(generate_token())
    }
}

/// A stored session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord<U = i64> {
    /// The user this session was minted for.
    pub user_id: U,
    /// When the session was minted; the base of `absolute_ttl`.
    pub created_at: DateTime<Utc>,
    /// When the session currently expires (extended while it stays active).
    pub expires_at: DateTime<Utc>,
    /// The last request that used the session; drives the renewal threshold.
    pub last_activity: DateTime<Utc>,
}

/// Storage back-end for sessions.
///
/// Implement against your own schema: the recommended table is
/// `sessions(session_id VARCHAR(64) UNIQUE, user_id, created_at, expires_at,
/// last_activity)`.
///
/// Every `id` this trait receives is a SHA-256 digest
/// ([`SessionId::digest`](SessionId::digest)), never the value the client holds
/// — store and compare it as-is. Times are UTC; adapt the columns to your
/// driver (a `DATETIME`/`TIMESTAMP` column, or epoch seconds). `remove` returns
/// whether a row existed, while `remove_by_user` returns the number of rows
/// deleted. Map storage failures to [`ApiError::internal`] or
/// [`ApiError::database`]; the middleware logs them and treats the request as
/// unauthenticated rather than exposing them to clients.
pub trait SessionStore: Send + Sync + 'static {
    /// The application's user id type.
    type UserId: Copy + Eq + Send + Sync + 'static;

    /// Persists a new session.
    fn create(
        &self,
        id: &str,
        user: Self::UserId,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        last_activity: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;

    /// Returns the session with this digest, if it exists.
    fn find(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<SessionRecord<Self::UserId>>, ApiError>> + Send;

    /// Updates the expiry and activity timestamps of an existing session.
    fn touch(
        &self,
        id: &str,
        expires_at: DateTime<Utc>,
        last_activity: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;

    /// Deletes one session; returns whether a row was deleted.
    fn remove(&self, id: &str) -> impl Future<Output = Result<bool, ApiError>> + Send;

    /// Deletes every session of a user (logout everywhere); returns the number
    /// of rows deleted.
    fn remove_by_user(
        &self,
        user: Self::UserId,
    ) -> impl Future<Output = Result<u64, ApiError>> + Send;
}

/// Session configuration and the cookie/start/end operations of an
/// application.
#[derive(Debug, Clone)]
pub struct SessionAuth<S> {
    store: S,
    cookie: CookieOptions,
    ttl: Duration,
    absolute_ttl: Option<Duration>,
}

impl<S> SessionAuth<S> {
    /// Constructs session auth from a store, cookie options and lifetimes.
    ///
    /// `ttl` is the sliding lifetime renewed while the session stays active;
    /// `absolute_ttl` optionally caps the total lifetime, after which the
    /// session dies no matter how active it is (`None` keeps it renewable
    /// forever).
    pub fn new(
        store: S,
        cookie: CookieOptions,
        ttl: Duration,
        absolute_ttl: Option<Duration>,
    ) -> Self {
        Self {
            store,
            cookie,
            ttl,
            absolute_ttl,
        }
    }

    /// Accessor for the wrapped store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The cookie the session id travels in.
    pub fn cookie(&self) -> &CookieOptions {
        &self.cookie
    }

    /// The sliding session TTL.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// The absolute session lifetime cap, if one is configured.
    pub fn absolute_ttl(&self) -> Option<Duration> {
        self.absolute_ttl
    }

    /// The instant `created_at` stops being renewable.
    ///
    /// `None` without an absolute cap. Saturates instead of overflowing for an
    /// absurd `absolute_ttl`, so the session simply never hits the cap.
    fn deadline(&self, created_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.absolute_ttl.map(|cap| {
            created_at
                .checked_add_signed(delta(cap))
                .unwrap_or(DateTime::<Utc>::MAX_UTC)
        })
    }

    /// The expiry a session created at `created_at` has at `now`: the sliding
    /// TTL, capped by the absolute deadline.
    fn expiry(&self, created_at: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
        let sliding = now
            .checked_add_signed(delta(self.ttl))
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
        match self.deadline(created_at) {
            Some(deadline) => sliding.min(deadline),
            None => sliding,
        }
    }

    /// Whether a stored session is dead at `now`, either by sliding expiry or
    /// by having reached its absolute deadline.
    fn is_dead<U>(&self, record: &SessionRecord<U>, now: DateTime<Utc>) -> bool {
        record.expires_at <= now
            || self
                .deadline(record.created_at)
                .is_some_and(|deadline| now >= deadline)
    }
}

impl<S: SessionStore> SessionAuth<S> {
    /// Creates a new session for `user` and returns its id.
    ///
    /// The id goes into the cookie via [`set_cookie_value`]; no cookie is set
    /// here, the login handler builds its response. Only the id's digest
    /// reaches the store.
    ///
    /// [`set_cookie_value`]: SessionAuth::set_cookie_value
    pub async fn start(&self, user: S::UserId) -> Result<SessionId, ApiError> {
        let id = SessionId::generate();
        let now = Utc::now();
        self.store
            .create(&id.digest(), user, now, self.expiry(now, now), now)
            .await?;
        Ok(id)
    }

    /// Deletes one session (logout).
    ///
    /// Idempotent: ending a session that is already gone is not an error.
    pub async fn end(&self, id: &SessionId) -> Result<(), ApiError> {
        if !self.store.remove(&id.digest()).await? {
            tracing::debug!("logout of an unknown session");
        }
        Ok(())
    }

    /// Deletes every session of a user (logout everywhere), returning how many
    /// were deleted.
    pub async fn end_for_user(&self, user: S::UserId) -> Result<u64, ApiError> {
        self.store.remove_by_user(user).await
    }

    /// The `Set-Cookie` header value establishing a session with a fresh
    /// `Max-Age` equal to the TTL.
    ///
    /// The attributes come from the configured [`CookieOptions`]. When an
    /// absolute cap is configured and shorter than the TTL, the `Max-Age` is
    /// the cap instead: the cookie then expires with the session `start` just
    /// created rather than outliving it.
    pub fn set_cookie_value(&self, id: &SessionId) -> HeaderValue {
        self.cookie_value(id.as_str(), self.fresh_max_age())
    }

    /// The lifetime `start` gives a fresh session, in seconds.
    fn fresh_max_age(&self) -> u64 {
        self.absolute_ttl
            .unwrap_or(self.ttl)
            .min(self.ttl)
            .as_secs()
    }

    /// The `Set-Cookie` header value that expires the session cookie
    /// (`Max-Age=0`).
    pub fn clear_cookie_value(&self) -> HeaderValue {
        self.cookie_value("", 0)
    }

    fn cookie_value(&self, value: &str, max_age: u64) -> HeaderValue {
        let mut cookie = Cookie::new(self.cookie.name.clone(), value.to_string());
        cookie.set_path(self.cookie.path.clone());
        cookie.set_http_only(self.cookie.http_only);
        cookie.set_same_site(self.cookie.same_site);
        if self.cookie.secure {
            cookie.set_secure(Some(true));
        }
        if let Some(domain) = &self.cookie.domain {
            cookie.set_domain(domain.clone());
        }
        cookie.set_max_age(Some(cookie::time::Duration::seconds(
            max_age.min(i64::MAX as u64) as i64,
        )));
        HeaderValue::from_str(&cookie.encoded().to_string())
            .unwrap_or_else(|_| HeaderValue::from_static(""))
    }
}

/// The authenticated principal extracted by [`session_layer`].
///
/// Parameterized by the store's [`UserId`](SessionStore::UserId), so a handler
/// reads the application's own id type (`SessionCtx<u64>`); it defaults to
/// `i64`.
#[derive(Debug, Clone)]
pub struct SessionCtx<U = i64> {
    /// The authenticated user's id.
    pub user_id: U,
}

impl<S, U> FromRequestParts<S> for SessionCtx<U>
where
    S: Send + Sync,
    U: Clone + Send + Sync + 'static,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<SessionCtx<U>>()
            .cloned()
            .ok_or_else(|| ApiError::unauthorized(texts().unauthorized.clone()))
    }
}

/// Optional variant of [`SessionCtx`]: yields `Some` when authenticated,
/// `None` otherwise, never rejecting.
///
/// A standalone newtype — Rust's orphan rules prevent implementing
/// `FromRequestParts<S>` for `Option<SessionCtx>` directly.
#[derive(Debug, Clone)]
pub struct OptionalSessionCtx<U = i64>(pub Option<SessionCtx<U>>);

impl<S, U> FromRequestParts<S> for OptionalSessionCtx<U>
where
    S: Send + Sync,
    U: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(OptionalSessionCtx(
            parts.extensions.get::<SessionCtx<U>>().cloned(),
        ))
    }
}

impl<S> FromRequestParts<S> for SessionId
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<SessionId>()
            .cloned()
            .ok_or_else(|| ApiError::unauthorized(texts().unauthorized.clone()))
    }
}

/// Returns true when a session should be extended: it has been idle for at
/// least half its TTL (the threshold prevents writing to the store on every
/// request, matching the reference implementation).
pub fn should_extend<U>(record: &SessionRecord<U>, ttl: Duration) -> bool {
    idle_for(record) >= delta(ttl / 2)
}

/// How long ago the session was last used (zero for a future timestamp, which
/// only clock skew can produce).
fn idle_for<U>(record: &SessionRecord<U>) -> TimeDelta {
    let idle = Utc::now().signed_duration_since(record.last_activity);
    if idle < TimeDelta::zero() {
        TimeDelta::zero()
    } else {
        idle
    }
}

/// A standard [`Duration`] as a chrono delta, saturating where chrono cannot
/// represent it (so an absurd TTL expires "never" instead of panicking).
fn delta(duration: Duration) -> TimeDelta {
    TimeDelta::from_std(duration).unwrap_or(TimeDelta::MAX)
}

/// The boxed future returned by the session middleware.
pub type SessionMiddlewareFuture = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;

/// The layer returned by [`session_layer`].
pub type SessionLayer<S> = FromFnLayer<
    fn(State<SessionAuth<S>>, Request, Next) -> SessionMiddlewareFuture,
    SessionAuth<S>,
    (State<SessionAuth<S>>, Request),
>;

/// Builds the cookie-session middleware layer.
///
/// The layer resolves the session cookie, validates the session against the
/// store (deleting dead ones) and stashes [`SessionCtx`] and [`SessionId`] in
/// the request extensions; after the handler it extends the session once the
/// half-TTL threshold is reached and refreshes the cookie `Max-Age`. Invalid or
/// dead sessions clear the stale browser cookie. The layer itself never
/// rejects; use the [`SessionCtx`] or [`OptionalSessionCtx`] extractors.
pub fn session_layer<S>(auth: SessionAuth<S>) -> SessionLayer<S>
where
    S: SessionStore + Clone + Send + Sync + 'static,
{
    middleware::from_fn_with_state(
        auth,
        session_middleware::<S>
            as fn(State<SessionAuth<S>>, Request, Next) -> SessionMiddlewareFuture,
    )
}

/// The middleware function backing [`session_layer`].
fn session_middleware<S>(
    State(auth): State<SessionAuth<S>>,
    mut request: Request,
    next: Next,
) -> SessionMiddlewareFuture
where
    S: SessionStore + Clone + Send + Sync + 'static,
{
    Box::pin(async move {
        let cookie = cookie_value(request.headers(), &auth.cookie().name);
        let session = cookie.as_deref().map(|raw| SessionId(raw.to_string()));
        let digest = session.as_ref().map(SessionId::digest);
        let now = Utc::now();
        let mut live: Option<SessionRecord<S::UserId>> = None;
        let mut lookup_error = false;

        if let (Some(id), Some(digest)) = (&session, &digest) {
            match auth.store().find(digest).await {
                Ok(Some(record)) if !auth.is_dead(&record, now) => {
                    request.extensions_mut().insert(SessionCtx {
                        user_id: record.user_id,
                    });
                    request.extensions_mut().insert(id.clone());
                    live = Some(record);
                }
                Ok(Some(_)) => {
                    // Dead (expired or past its absolute cap): drop it and let
                    // the response clear the cookie below.
                    if let Err(err) = auth.store().remove(digest).await {
                        tracing::warn!(error = %err, "failed to delete an expired session");
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    lookup_error = true;
                    tracing::warn!(
                        error = %err,
                        "session lookup failed; treating as unauthenticated"
                    );
                }
            }
        }

        let mut response = next.run(request).await;

        // A handler may set the session cookie itself (login rotation,
        // logout) — respect it: RFC 6265 last-wins would let our renew/clear
        // clobber it, and on transient lookup errors we must not clear a
        // possibly-valid cookie either.
        let handler_owns_cookie = sets_cookie(response.headers(), &auth.cookie().name);

        match (&live, &session, &digest) {
            (Some(record), Some(id), Some(digest)) => {
                if should_extend(record, auth.ttl()) && !handler_owns_cookie {
                    // The session survived the lookup, so its absolute deadline
                    // is still ahead: the renewal below can only move forward.
                    // Time is read again here — the lookup's `now` predates the
                    // handler, and the browser starts counting `Max-Age` when
                    // the response arrives.
                    let renewed_at = Utc::now();
                    let renewed = auth.expiry(record.created_at, renewed_at);
                    match auth.store().touch(digest, renewed, renewed_at).await {
                        Ok(()) => {
                            // The cookie expires with the session, so the
                            // client drops it at the absolute cap too.
                            let max_age = (renewed - renewed_at).num_seconds().max(1) as u64;
                            response.headers_mut().append(
                                header::SET_COOKIE,
                                auth.cookie_value(id.as_str(), max_age),
                            );
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "session renewal failed");
                        }
                    }
                }
            }
            _ if session.is_some() && !lookup_error && !handler_owns_cookie => {
                // Stale cookie (unknown or dead session): expire it.
                response
                    .headers_mut()
                    .append(header::SET_COOKIE, auth.clear_cookie_value());
            }
            _ => {}
        }

        response
    })
}

/// Extracts a cookie's value from the `Cookie` request header.
///
/// Parsing goes through the `cookie` crate rather than string surgery:
///
/// - Percent-encoded values are decoded (`split_parse_encoded`), symmetric with
///   the [`encoded()`](cookie::Cookie::encoded) form [`SessionAuth`] writes, so
///   an id that needed escaping on the way out is found again on the way in.
/// - A value wrapped in double quotes is unquoted, as RFC 6265's
///   `cookie-value` grammar allows; an unpaired quote is left alone, so a
///   malformed cookie simply fails to match a session.
/// - A value-less `name=` yields an empty string rather than being dropped: no
///   digest matches it, so the middleware treats it as a stale cookie and
///   clears it.
/// - Segments that fail to parse (no `=`, an empty name) are skipped, and other
///   cookies are ignored.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let value = Cookie::split_parse_encoded(raw)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_string())?;
    Some(unquote(&value).to_string())
}

/// Strips one pair of surrounding double quotes, leaving a lone quote alone.
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value)
}

/// Whether the response already sets the session cookie.
fn sets_cookie(headers: &HeaderMap, name: &str) -> bool {
    headers.get_all(header::SET_COOKIE).iter().any(|value| {
        value
            .to_str()
            .ok()
            .and_then(|raw| raw.split(';').next())
            .and_then(|pair| pair.split_once('='))
            .map(|(cookie, _)| cookie.trim() == name)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use http_body_util::BodyExt;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    type Store = Arc<Mutex<HashMap<String, SessionRecord<u64>>>>;

    #[derive(Clone, Default)]
    struct InMemorySessionStore(Store);

    impl InMemorySessionStore {
        fn insert(&self, digest: &str, record: SessionRecord<u64>) {
            self.0.lock().insert(digest.to_string(), record);
        }

        fn get(&self, digest: &str) -> Option<SessionRecord<u64>> {
            self.0.lock().get(digest).cloned()
        }

        fn contains(&self, digest: &str) -> bool {
            self.0.lock().contains_key(digest)
        }

        fn keys(&self) -> Vec<String> {
            self.0.lock().keys().cloned().collect()
        }
    }

    impl SessionStore for InMemorySessionStore {
        type UserId = u64;

        async fn create(
            &self,
            id: &str,
            user: u64,
            created_at: DateTime<Utc>,
            expires_at: DateTime<Utc>,
            last_activity: DateTime<Utc>,
        ) -> Result<(), ApiError> {
            self.insert(
                id,
                SessionRecord {
                    user_id: user,
                    created_at,
                    expires_at,
                    last_activity,
                },
            );
            Ok(())
        }

        async fn find(&self, id: &str) -> Result<Option<SessionRecord<u64>>, ApiError> {
            Ok(self.get(id))
        }

        async fn touch(
            &self,
            id: &str,
            expires_at: DateTime<Utc>,
            last_activity: DateTime<Utc>,
        ) -> Result<(), ApiError> {
            if let Some(record) = self.0.lock().get_mut(id) {
                record.expires_at = expires_at;
                record.last_activity = last_activity;
            }
            Ok(())
        }

        async fn remove(&self, id: &str) -> Result<bool, ApiError> {
            Ok(self.0.lock().remove(id).is_some())
        }

        async fn remove_by_user(&self, user: u64) -> Result<u64, ApiError> {
            let mut store = self.0.lock();
            let before = store.len() as u64;
            store.retain(|_, record| record.user_id != user);
            Ok(before - store.len() as u64)
        }
    }

    const TTL: Duration = Duration::from_secs(120);

    fn auth<S: SessionStore + Clone>(store: S, absolute_ttl: Option<Duration>) -> SessionAuth<S> {
        SessionAuth::new(store, CookieOptions::new("sid"), TTL, absolute_ttl)
    }

    fn record(
        user_id: u64,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        last_activity: DateTime<Utc>,
    ) -> SessionRecord<u64> {
        SessionRecord {
            user_id,
            created_at,
            expires_at,
            last_activity,
        }
    }

    fn me_app(auth: SessionAuth<InMemorySessionStore>) -> Router {
        Router::new()
            .route(
                "/me",
                get(|SessionCtx { user_id }: SessionCtx<u64>| async move { user_id.to_string() }),
            )
            .layer(session_layer(auth))
    }

    fn req_get(uri: &str, cookie: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(uri);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        builder.body(Body::empty()).expect("request builds")
    }

    async fn drive(app: Router, req: Request<Body>) -> (StatusCode, Response) {
        let response = app.oneshot(req).await.expect("request succeeds");
        (response.status(), response)
    }

    async fn body_text(response: Response) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("utf-8 body")
    }

    fn set_cookies(response: &Response) -> String {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap_or("").to_string())
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn max_age(cookie: &str) -> u64 {
        cookie
            .split(';')
            .find_map(|part| part.trim().strip_prefix("Max-Age="))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("no Max-Age in {cookie}"))
    }

    #[tokio::test]
    async fn missing_cookie_is_401() {
        let app = me_app(auth(InMemorySessionStore::default(), None));
        let (status, response) = drive(app, req_get("/me", None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let json: serde_json::Value =
            serde_json::from_str(&body_text(response).await).expect("error body is json");
        assert_eq!(json["code"], 401);
        assert_eq!(json["message"], texts().unauthorized.as_ref());
        assert_eq!(json["data"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn valid_session_reaches_handler() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        store.insert(&hash_token("s1"), record(7, now, now + delta(TTL), now));
        let app = me_app(auth(store, None));

        let (status, response) = drive(app, req_get("/me", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "7");
    }

    #[tokio::test]
    async fn expired_session_is_removed_and_cookie_cleared() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        store.insert(
            &hash_token("s1"),
            record(
                7,
                now - TimeDelta::seconds(200),
                now - TimeDelta::seconds(10),
                now,
            ),
        );
        let app = me_app(auth(store.clone(), None));

        let (status, response) = drive(app, req_get("/me", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(set_cookies(&response).contains("Max-Age=0"));
        assert!(!store.contains(&hash_token("s1")));
    }

    #[tokio::test]
    async fn half_ttl_elapsed_renews_expiry_and_cookie() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        store.insert(
            &hash_token("s1"),
            record(
                7,
                now - TimeDelta::seconds(120),
                now + TimeDelta::seconds(59),
                now - TimeDelta::seconds(61),
            ),
        );
        let app = me_app(auth(store.clone(), None));

        let (status, response) = drive(app, req_get("/me", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::OK);

        let renewed = store.get(&hash_token("s1")).expect("session kept");
        assert!(
            renewed.expires_at > now + TimeDelta::seconds(110),
            "expiry not renewed: {:?}",
            renewed.expires_at
        );
        assert_eq!(max_age(&set_cookies(&response)), TTL.as_secs());
    }

    #[tokio::test]
    async fn optional_extractor_never_rejects() {
        let app = Router::new()
            .route(
                "/any",
                get(
                    |OptionalSessionCtx(ctx): OptionalSessionCtx<u64>| async move {
                        ctx.is_some().to_string()
                    },
                ),
            )
            .layer(session_layer(auth(InMemorySessionStore::default(), None)));

        let (status, response) = drive(app, req_get("/any", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "false");
    }

    #[tokio::test]
    async fn unknown_session_is_401_and_cookie_cleared() {
        let app = me_app(auth(InMemorySessionStore::default(), None));
        let (status, response) = drive(app, req_get("/me", Some("sid=nope"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(set_cookies(&response).contains("Max-Age=0"));
    }

    #[tokio::test]
    async fn handler_set_cookie_is_not_clobbered_by_renewal() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        // Old-but-valid session that would trigger renewal (past half TTL).
        store.insert(
            &hash_token("s1"),
            record(
                7,
                now - TimeDelta::seconds(120),
                now + TimeDelta::seconds(59),
                now - TimeDelta::seconds(61),
            ),
        );
        let auth = auth(store, None);
        let handler_auth = auth.clone();
        let app = Router::new()
            .route(
                "/login",
                get(move |SessionCtx { user_id }: SessionCtx<u64>| {
                    let value = handler_auth.set_cookie_value(&SessionId("s2".to_string()));
                    async move { ([(header::SET_COOKIE, value)], format!("ok {user_id}")) }
                }),
            )
            .layer(session_layer(auth));

        let (status, response) = drive(app, req_get("/login", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::OK);
        let cookie = set_cookies(&response);
        assert_eq!(body_text(response).await, "ok 7");
        assert!(
            !cookie.contains("s1"),
            "stale session id clobbered the handler cookie: {cookie}"
        );
        assert!(
            cookie.contains("sid=s2"),
            "handler cookie missing: {cookie}"
        );
        assert!(
            !cookie.contains("Max-Age=0"),
            "clear cookie clobbered the handler cookie: {cookie}"
        );
    }

    #[derive(Clone)]
    struct FailingStore;

    impl SessionStore for FailingStore {
        type UserId = u64;

        async fn create(
            &self,
            _id: &str,
            _user: u64,
            _created_at: DateTime<Utc>,
            _expires_at: DateTime<Utc>,
            _last_activity: DateTime<Utc>,
        ) -> Result<(), ApiError> {
            Ok(())
        }

        async fn find(&self, _id: &str) -> Result<Option<SessionRecord<u64>>, ApiError> {
            Err(ApiError::internal("lookup failed"))
        }

        async fn touch(
            &self,
            _id: &str,
            _expires_at: DateTime<Utc>,
            _last_activity: DateTime<Utc>,
        ) -> Result<(), ApiError> {
            Ok(())
        }

        async fn remove(&self, _id: &str) -> Result<bool, ApiError> {
            Ok(false)
        }

        async fn remove_by_user(&self, _user: u64) -> Result<u64, ApiError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn lookup_error_does_not_clear_cookie() {
        let app = Router::new()
            .route(
                "/me",
                get(|SessionCtx { user_id }: SessionCtx<u64>| async move { user_id.to_string() }),
            )
            .layer(session_layer(auth(FailingStore, None)));

        let (status, response) = drive(app, req_get("/me", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let cookie = set_cookies(&response);
        assert!(
            !cookie.contains("Max-Age=0"),
            "transient lookup error must not clear the client cookie: {cookie}"
        );
    }

    #[tokio::test]
    async fn store_sees_only_digests_and_the_extractor_the_raw_id() {
        let store = InMemorySessionStore::default();
        let auth = auth(store.clone(), None);
        let id = auth.start(42).await.expect("start");

        assert_eq!(
            store.keys(),
            vec![id.digest()],
            "the store must key sessions by digest"
        );
        assert_eq!(id.digest().len(), 64);
        assert!(
            !store.contains(id.as_str()),
            "the raw id must never reach the store"
        );

        // The extractor hands the handler the raw id, which is what logout
        // needs to hand back to `SessionAuth::end`.
        let seen: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let captured = seen.clone();
        let app = Router::new()
            .route(
                "/id",
                get(move |id: SessionId| {
                    let captured = captured.clone();
                    async move {
                        *captured.lock() = id.0;
                        "ok"
                    }
                }),
            )
            .layer(session_layer(auth));

        let (status, _) = drive(app, req_get("/id", Some(&format!("sid={}", id.as_str())))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(*seen.lock(), id.as_str());
    }

    #[tokio::test]
    async fn start_end_and_end_for_user_touch_only_the_store() {
        let store = InMemorySessionStore::default();
        let auth = auth(store.clone(), None);

        let first = auth.start(7).await.expect("start");
        let second = auth.start(7).await.expect("start");
        let other = auth.start(8).await.expect("start");
        assert_eq!(store.keys().len(), 3);
        assert_ne!(first.digest(), second.digest(), "ids must be unique");

        auth.end(&first).await.expect("end");
        assert!(!store.contains(&first.digest()));
        auth.end(&first).await.expect("end is idempotent");

        assert_eq!(auth.end_for_user(7).await.expect("end_for_user"), 1);
        assert!(store.contains(&other.digest()));
    }

    #[tokio::test]
    async fn cookie_attributes_default_to_secure_http_only_lax() {
        let id = SessionId("raw".to_string());
        let secure = auth(InMemorySessionStore::default(), None);
        let cookie = secure
            .set_cookie_value(&id)
            .to_str()
            .expect("ascii cookie")
            .to_string();
        assert!(cookie.contains("sid=raw"), "{cookie}");
        assert!(cookie.contains("Secure"), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Path=/"), "{cookie}");
        assert_eq!(max_age(&cookie), TTL.as_secs());
        assert!(!cookie.contains("Max-Age=0"), "{cookie}");

        let insecure = SessionAuth::new(
            InMemorySessionStore::default(),
            CookieOptions::new("sid").insecure(),
            TTL,
            None,
        );
        let cookie = insecure
            .set_cookie_value(&id)
            .to_str()
            .expect("ascii cookie")
            .to_string();
        assert!(!cookie.contains("Secure"), "{cookie}");
        assert_eq!(max_age(insecure.clear_cookie_value().to_str().unwrap()), 0);
    }

    #[tokio::test]
    async fn absolute_ttl_kills_a_session_that_is_still_active() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        // The sliding expiry is 60s away and the session was active a minute
        // ago, but it is two hours old under a one-hour cap.
        store.insert(
            &hash_token("raw"),
            record(
                7,
                now - TimeDelta::hours(2),
                now + TimeDelta::seconds(60),
                now - TimeDelta::seconds(61),
            ),
        );
        let app = me_app(auth(store.clone(), Some(Duration::from_secs(3600))));

        let (status, response) = drive(app, req_get("/me", Some("sid=raw"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(set_cookies(&response).contains("Max-Age=0"));
        assert!(
            !store.contains(&hash_token("raw")),
            "the session past its cap must be deleted"
        );
    }

    #[tokio::test]
    async fn absolute_ttl_caps_the_renewal_and_the_cookie() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        // Renewal is due (idle past half the TTL) and the cap is 30s away, so
        // the renewed expiry must stop at the cap instead of taking the full
        // TTL — and the cookie must shrink with it.
        let created_at = now - TimeDelta::seconds(3570);
        let deadline = created_at + TimeDelta::seconds(3600);
        store.insert(
            &hash_token("raw"),
            record(
                7,
                created_at,
                now + TimeDelta::seconds(30),
                now - TimeDelta::seconds(61),
            ),
        );
        let app = me_app(auth(store.clone(), Some(Duration::from_secs(3600))));

        let (status, response) = drive(app, req_get("/me", Some("sid=raw"))).await;
        assert_eq!(status, StatusCode::OK);

        let renewed = store.get(&hash_token("raw")).expect("session kept");
        assert!(
            renewed.expires_at <= deadline,
            "renewal went past the cap: {:?} > {deadline:?}",
            renewed.expires_at
        );
        assert!(
            renewed.expires_at > now,
            "renewal must still extend a live session"
        );
        let cookie = set_cookies(&response);
        let age = max_age(&cookie);
        assert!(
            (25..=30).contains(&age),
            "cookie must expire with the cap, got Max-Age={age}: {cookie}"
        );
    }

    #[tokio::test]
    async fn a_recently_active_session_is_not_renewed() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        let expires_at = now + TimeDelta::seconds(60);
        // Active ten seconds ago: well inside the half-TTL threshold.
        store.insert(
            &hash_token("raw"),
            record(
                7,
                now - TimeDelta::seconds(120),
                expires_at,
                now - TimeDelta::seconds(10),
            ),
        );
        let app = me_app(auth(store.clone(), None));

        let (status, response) = drive(app, req_get("/me", Some("sid=raw"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            store.get(&hash_token("raw")).expect("kept").expires_at,
            expires_at,
            "a recent session must not be touched"
        );
        assert!(
            set_cookies(&response).is_empty(),
            "a recent session must not re-issue the cookie: {}",
            set_cookies(&response)
        );
    }

    #[test]
    fn login_cookie_never_outlives_a_shorter_absolute_cap() {
        let id = SessionId("raw".to_string());

        let capped = SessionAuth::new(
            InMemorySessionStore::default(),
            CookieOptions::new("sid"),
            Duration::from_secs(86_400),
            Some(Duration::from_secs(3600)),
        );
        assert_eq!(
            max_age(capped.set_cookie_value(&id).to_str().unwrap()),
            3600
        );

        let uncapped = auth(InMemorySessionStore::default(), None);
        assert_eq!(
            max_age(uncapped.set_cookie_value(&id).to_str().unwrap()),
            TTL.as_secs()
        );
    }

    #[test]
    fn session_id_debug_prints_the_digest_not_the_credential() {
        let id = SessionId("a-bearer-credential".to_string());
        let printed = format!("{id:?}");

        assert!(!printed.contains("a-bearer-credential"), "{printed}");
        assert_eq!(printed, format!("SessionId({}…)", &id.digest()[..8]));
    }

    /// A request header with the given `Cookie` value.
    fn headers(raw: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(raw).expect("a valid header"),
        );
        headers
    }

    #[test]
    fn cookie_values_are_parsed_per_rfc_6265() {
        // Percent-encoded values are decoded, symmetrically with `encoded()`.
        assert_eq!(
            cookie_value(&headers("sid=a%2Bb%2Fc"), "sid").as_deref(),
            Some("a+b/c")
        );
        // A quoted value is unquoted; the quotes are not part of the id.
        assert_eq!(
            cookie_value(&headers("sid=\"abc\""), "sid").as_deref(),
            Some("abc")
        );
        // An unpaired quote is not a quoted value, and stays as it is.
        assert_eq!(
            cookie_value(&headers("sid=\"abc"), "sid").as_deref(),
            Some("\"abc")
        );
        assert_eq!(
            cookie_value(&headers("sid=abc\""), "sid").as_deref(),
            Some("abc\"")
        );
        // A value-less cookie is kept as an empty string, not dropped: the
        // middleware then clears the stale cookie instead of ignoring it.
        assert_eq!(cookie_value(&headers("sid="), "sid").as_deref(), Some(""));
        // Other cookies, name prefixes and malformed segments are skipped.
        assert_eq!(
            cookie_value(&headers("other=1; sid=abc; more=2"), "sid").as_deref(),
            Some("abc")
        );
        assert_eq!(cookie_value(&headers("sid_suffix=abc"), "sid"), None);
        assert_eq!(
            cookie_value(&headers("garbage; sid=abc"), "sid").as_deref(),
            Some("abc")
        );
        assert_eq!(cookie_value(&headers("=abc"), "sid"), None);
        assert_eq!(cookie_value(&headers(""), "sid"), None);
    }

    #[tokio::test]
    async fn an_escaped_session_id_round_trips_through_the_cookie() {
        let now = Utc::now();
        let store = InMemorySessionStore::default();
        let auth = auth(store.clone(), None);
        // An id that has to be escaped on the way out, to exercise the
        // encoder/decoder pair rather than the identity path.
        let id = SessionId("ab+cd/ef".to_string());
        let set_cookie = auth
            .set_cookie_value(&id)
            .to_str()
            .expect("ascii cookie")
            .to_string();
        assert!(
            set_cookie.contains('%'),
            "the writer must escape this value: {set_cookie}"
        );

        let request_cookie = set_cookie
            .split(';')
            .next()
            .expect("a name=value pair")
            .to_string();
        store.insert(&id.digest(), record(7, now, now + delta(TTL), now));

        let app = me_app(auth);
        let (status, response) = drive(app, req_get("/me", Some(&request_cookie))).await;
        assert_eq!(status, StatusCode::OK, "cookie: {request_cookie}");
        assert_eq!(body_text(response).await, "7");
    }

    #[tokio::test]
    async fn an_empty_or_malformed_cookie_is_treated_as_stale() {
        let app = me_app(auth(InMemorySessionStore::default(), None));

        let (status, response) = drive(app, req_get("/me", Some("sid="))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(set_cookies(&response).contains("Max-Age=0"));
    }
}
