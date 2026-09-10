//! Server-side cookie sessions with sliding renewal.
//!
//! A session is an opaque, randomly generated id stored in an
//! `HttpOnly; SameSite=Lax; Path=/` cookie. The server keeps the trust
//! boundary: [`SessionStore`] persists a row per session (`session_id`
//! unique, `user_id`, `expires_at`, `last_activity` — table is
//! app-owned), and [`SessionCtx`] only exists when the middleware found a
//! live session.
//!
//! Layering, matching the reference implementation:
//! - [`session_layer`] middleware resolves the cookie, checks expiry
//!   (deleting expired sessions), and performs sliding renewal after the
//!   response: once a session has lived past half its TTL, the TTL is
//!   extended and the cookie's `Max-Age` is refreshed in the response.
//!   The middleware never rejects a request — authentication decisions are
//!   the [`SessionCtx`] extractor's job.
//! - [`SessionAuth::start`]/[`SessionAuth::end`] create sessions at login
//!   and delete them at logout; [`SessionAuth::set_cookie_value`] produces
//!   the `Set-Cookie` header value for the login response.
//!
//! ```no_run
//! use axum::{Router, routing::get};
//! use parking_lot::Mutex;
//! use std::{collections::HashMap, sync::Arc, time::Duration};
//! use vivarium_web::{SessionAuth, SessionCtx, SessionRecord, SessionStore, session_layer};
//!
//! #[derive(Clone, Default)]
//! struct MyStore(Arc<Mutex<HashMap<String, SessionRecord>>>);
//!
//! impl SessionStore for MyStore {
//!     async fn create(&self, session_id: &str, user_id: i64, expires_at: std::time::SystemTime,
//!         last_activity: std::time::SystemTime) -> Result<(), vivarium_web::ApiError> {
//!         self.0.lock().insert(session_id.to_string(),
//!             SessionRecord { user_id, expires_at, last_activity });
//!         Ok(())
//!     }
//!     async fn find(&self, session_id: &str)
//!         -> Result<Option<SessionRecord>, vivarium_web::ApiError> {
//!         Ok(self.0.lock().get(session_id).cloned())
//!     }
//!     async fn touch(&self, session_id: &str, expires_at: std::time::SystemTime,
//!         last_activity: std::time::SystemTime) -> Result<(), vivarium_web::ApiError> {
//!         if let Some(record) = self.0.lock().get_mut(session_id) {
//!             record.expires_at = expires_at;
//!             record.last_activity = last_activity;
//!         }
//!         Ok(())
//!     }
//!     async fn remove(&self, session_id: &str) -> Result<(), vivarium_web::ApiError> {
//!         self.0.lock().remove(session_id);
//!         Ok(())
//!     }
//!     async fn remove_by_user(&self, user_id: i64) -> Result<(), vivarium_web::ApiError> {
//!         self.0.lock().retain(|_, r| r.user_id != user_id);
//!         Ok(())
//!     }
//! }
//!
//! async fn me(SessionCtx { user_id }: SessionCtx) -> String { user_id.to_string() }
//!
//! # async fn example() {
//! let auth = SessionAuth::new(MyStore::default(), "sid", Duration::from_secs(3600));
//! let app: Router = Router::new()
//!     .route("/me", get(me))
//!     .layer(session_layer(auth));
//! # }
//! ```

use std::pin::Pin;
use std::time::{Duration, SystemTime};

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header;
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::{self, FromFnLayer, Next};
use axum::response::Response;
use cookie::{Cookie, SameSite};
use uuid::Uuid;

use crate::error::ApiError;
use crate::texts::texts;

/// A live session as stored server-side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// The authenticated user's id.
    pub user_id: i64,
    /// The instant this session expires.
    pub expires_at: SystemTime,
    /// The instant the session was last active; drives sliding renewal.
    pub last_activity: SystemTime,
}

/// Storage back-end for sessions.
///
/// Implement against your own schema: the recommended table is
/// `sessions(session_id UNIQUE, user_id, expires_at, last_activity)`
/// (adapt the time columns to your driver — epoch seconds is the portable
/// choice). Map storage failures to [`ApiError::internal`] or
/// [`ApiError::database`]; the middleware never exposes them to clients.
pub trait SessionStore: Send + Sync + 'static {
    /// Persists a new session.
    fn create<'a>(
        &'a self,
        session_id: &'a str,
        user_id: i64,
        expires_at: SystemTime,
        last_activity: SystemTime,
    ) -> impl Future<Output = Result<(), ApiError>> + Send + 'a;

    /// Returns the session with this id, if it exists.
    fn find<'a>(
        &'a self,
        session_id: &'a str,
    ) -> impl Future<Output = Result<Option<SessionRecord>, ApiError>> + Send + 'a;

    /// Updates the expiry and activity timestamps of an existing session.
    fn touch<'a>(
        &'a self,
        session_id: &'a str,
        expires_at: SystemTime,
        last_activity: SystemTime,
    ) -> impl Future<Output = Result<(), ApiError>> + Send + 'a;

    /// Deletes one session.
    fn remove<'a>(
        &'a self,
        session_id: &'a str,
    ) -> impl Future<Output = Result<(), ApiError>> + Send + 'a;

    /// Deletes every session of a user (logout everywhere).
    fn remove_by_user(
        &self,
        user_id: i64,
    ) -> impl Future<Output = Result<(), ApiError>> + Send + '_;
}

/// Session configuration and the cookie/start/end operations of an
/// application.
#[derive(Clone)]
pub struct SessionAuth<S> {
    store: S,
    cookie_name: String,
    ttl: Duration,
}

impl<S> SessionAuth<S> {
    /// Constructs session auth with the given store, cookie name and TTL.
    pub fn new(store: S, cookie_name: impl Into<String>, ttl: Duration) -> Self {
        Self {
            store,
            cookie_name: cookie_name.into(),
            ttl,
        }
    }

    /// Accessor for the wrapped store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The cookie name holding the session id.
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    /// The session TTL; renewed sessions keep the same TTL.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// The instant a fresh session expires (`now + ttl`).
    ///
    /// Falls back to `now` if the addition would overflow (absurdly large
    /// TTL): the session is then immediately expired instead of panicking.
    pub fn expires_at(&self) -> SystemTime {
        let now = SystemTime::now();
        now.checked_add(self.ttl).unwrap_or(now)
    }

    /// Creates a new session for `user_id` and returns its opaque id.
    ///
    /// The id goes into the cookie via [`set_cookie_value`]. No cookie is set
    /// here; the handler builds the login response.
    ///
    /// [`set_cookie_value`]: SessionAuth::set_cookie_value
    pub async fn start(&self, user_id: i64) -> Result<String, ApiError>
    where
        S: SessionStore,
    {
        let session_id = Uuid::new_v4().to_string();
        let now = SystemTime::now();
        self.store
            .create(&session_id, user_id, self.expires_at(), now)
            .await?;
        Ok(session_id)
    }

    /// Deletes one session (logout).
    pub async fn end(&self, session_id: &str) -> Result<(), ApiError>
    where
        S: SessionStore,
    {
        self.store.remove(session_id).await
    }

    /// Deletes every session of a user (logout everywhere).
    pub async fn end_for_user(&self, user_id: i64) -> Result<(), ApiError>
    where
        S: SessionStore,
    {
        self.store.remove_by_user(user_id).await
    }

    /// The `Set-Cookie` header value establishing a session with a fresh
    /// `Max-Age` equal to the TTL.
    pub fn set_cookie_value(&self, session_id: &str) -> HeaderValue {
        self.cookie_value(session_id, self.ttl.as_secs().min(i64::MAX as u64))
    }

    /// The `Set-Cookie` header value that expires the session cookie
    /// (`Max-Age=0`).
    pub fn clear_cookie_value(&self) -> HeaderValue {
        self.cookie_value("", 0)
    }

    fn cookie_value(&self, value: &str, max_age: u64) -> HeaderValue {
        let mut cookie = Cookie::new(self.cookie_name.clone(), value.to_string());
        cookie.set_path("/");
        cookie.set_http_only(true);
        cookie.set_same_site(SameSite::Lax);
        cookie.set_max_age(Some(cookie::time::Duration::seconds(max_age as i64)));
        HeaderValue::from_str(&cookie.encoded().to_string())
            .unwrap_or_else(|_| HeaderValue::from_static(""))
    }
}

/// The authenticated principal extracted by [`session_layer`].
#[derive(Debug, Clone)]
pub struct SessionCtx {
    /// The authenticated user's id.
    pub user_id: i64,
}

impl<S: Send + Sync> FromRequestParts<S> for SessionCtx {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<SessionCtx>()
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
pub struct OptionalSessionCtx(pub Option<SessionCtx>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalSessionCtx {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(OptionalSessionCtx(
            parts.extensions.get::<SessionCtx>().cloned(),
        ))
    }
}

/// Returns true when a session has lived past half its TTL and should be
/// extended with the full TTL again (the `last_activity` threshold prevents
/// renewing on every request, matching the reference implementation).
pub fn should_extend(record: &SessionRecord, ttl: Duration) -> bool {
    record.last_activity.elapsed().unwrap_or(Duration::ZERO) >= ttl / 2
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
/// store (deleting expired ones) and stashes a [`SessionCtx`] in the request
/// extensions; after the handler, it extends the session once the half-TTL
/// threshold is reached and refreshes the cookie `Max-Age`. Invalid or
/// expired sessions clear the stale browser cookie. The layer itself never
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
        let session_id = parse_cookie(request.headers(), auth.cookie_name()).map(str::to_string);
        let now = SystemTime::now();
        let mut found: Option<(String, SessionRecord)> = None;
        let mut lookup_error = false;

        if let Some(id) = session_id.as_deref() {
            match auth.store().find(id).await {
                Ok(Some(record)) if record.expires_at > now => {
                    found = Some((id.to_string(), record.clone()));
                    request.extensions_mut().insert(SessionCtx {
                        user_id: record.user_id,
                    });
                }
                Ok(Some(_)) => {
                    // Expired: drop it (best effort) and clear the cookie below.
                    if let Err(err) = auth.store().remove(id).await {
                        tracing::warn!(error = %err, "failed to delete expired session");
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
        let handler_owns_cookie =
            response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .any(|value| {
                    value
                        .to_str()
                        .ok()
                        .and_then(|raw| raw.split(';').next())
                        .and_then(|pair| pair.split_once('='))
                        .map(|(name, _)| name.trim() == auth.cookie_name())
                        .unwrap_or(false)
                });

        match &found {
            Some((id, record)) => {
                if should_extend(record, auth.ttl()) && !handler_owns_cookie {
                    match auth.store().touch(id, auth.expires_at(), now).await {
                        Ok(()) => {
                            response
                                .headers_mut()
                                .append(header::SET_COOKIE, auth.set_cookie_value(id));
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "session renewal failed");
                        }
                    }
                }
            }
            None if session_id.is_some() && !lookup_error && !handler_owns_cookie => {
                // Stale cookie (unknown or expired session): expire it.
                response
                    .headers_mut()
                    .append(header::SET_COOKIE, auth.clear_cookie_value());
            }
            None => {}
        }

        response
    })
}

/// Extracts a cookie's value from the `Cookie` request header.
fn parse_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').map(str::trim).find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
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

    #[derive(Clone, Default)]
    struct InMemorySessionStore(Arc<Mutex<HashMap<String, SessionRecord>>>);

    impl SessionStore for InMemorySessionStore {
        async fn create(
            &self,
            session_id: &str,
            user_id: i64,
            expires_at: SystemTime,
            last_activity: SystemTime,
        ) -> Result<(), ApiError> {
            self.0.lock().insert(
                session_id.to_string(),
                SessionRecord {
                    user_id,
                    expires_at,
                    last_activity,
                },
            );
            Ok(())
        }

        async fn find(&self, session_id: &str) -> Result<Option<SessionRecord>, ApiError> {
            Ok(self.0.lock().get(session_id).cloned())
        }

        async fn touch(
            &self,
            session_id: &str,
            expires_at: SystemTime,
            last_activity: SystemTime,
        ) -> Result<(), ApiError> {
            if let Some(record) = self.0.lock().get_mut(session_id) {
                record.expires_at = expires_at;
                record.last_activity = last_activity;
            }
            Ok(())
        }

        async fn remove(&self, session_id: &str) -> Result<(), ApiError> {
            self.0.lock().remove(session_id);
            Ok(())
        }

        async fn remove_by_user(&self, user_id: i64) -> Result<(), ApiError> {
            self.0.lock().retain(|_, r| r.user_id != user_id);
            Ok(())
        }
    }

    const TTL: Duration = Duration::from_secs(120);

    fn auth<S: SessionStore + Clone>(store: S) -> SessionAuth<S> {
        SessionAuth::new(store, "sid", TTL)
    }

    fn me_app(store: InMemorySessionStore) -> Router {
        Router::new()
            .route(
                "/me",
                get(|SessionCtx { user_id }: SessionCtx| async move { user_id.to_string() }),
            )
            .layer(session_layer(auth(store)))
    }

    fn req_get(uri: &str, cookie: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(uri);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        builder.body(Body::empty()).unwrap()
    }

    async fn drive(app: Router, req: Request<Body>) -> (StatusCode, Response) {
        let response = app.oneshot(req).await.expect("request succeeds");
        let status = response.status();
        (status, response)
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
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[tokio::test]
    async fn missing_cookie_is_401() {
        let app = me_app(InMemorySessionStore::default());
        let (status, response) = drive(app, req_get("/me", None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let body = body_text(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).expect("error body is json");
        assert_eq!(json["code"], 401);
        assert_eq!(json["message"], texts().unauthorized.as_ref());
        assert_eq!(json["data"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn valid_session_reaches_handler() {
        let now = SystemTime::now();
        let store = InMemorySessionStore::default();
        store.create("s1", 7, now + TTL, now).await.expect("create");
        let app = me_app(store);

        let req = Request::builder()
            .uri("/me")
            .header(header::COOKIE, "sid=s1")
            .body(Body::empty())
            .unwrap();
        let (status, response) = drive(app, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "7");
    }

    #[tokio::test]
    async fn expired_session_is_removed_and_cookie_cleared() {
        let now = SystemTime::now();
        let store = InMemorySessionStore::default();
        store
            .create(
                "s1",
                7,
                now - Duration::from_secs(10),
                now - Duration::from_secs(130),
            )
            .await
            .expect("create");
        let app = me_app(store.clone());

        let (status, response) = drive(app, req_get("/me", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let set_cookie = set_cookies(&response);
        assert!(set_cookie.contains("Max-Age=0"), "set-cookie: {set_cookie}");
        assert!(store.0.lock().get("s1").is_none());
    }

    #[tokio::test]
    async fn half_ttl_elapsed_renews_expiry_and_cookie() {
        let now = SystemTime::now();
        let store = InMemorySessionStore::default();
        store
            .create(
                "s1",
                7,
                now + Duration::from_secs(59),
                now - Duration::from_secs(61),
            )
            .await
            .expect("create");
        let app = me_app(store.clone());

        let (status, response) = drive(app, req_get("/me", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::OK);

        let updated = store.0.lock().get("s1").cloned().expect("session kept");
        assert!(
            updated.expires_at > now + Duration::from_secs(110),
            "expiry not renewed: {:?}",
            updated.expires_at
        );
        let set_cookie = set_cookies(&response);
        assert!(
            set_cookie.contains("Max-Age=120"),
            "set-cookie: {set_cookie}"
        );
    }

    #[tokio::test]
    async fn optional_extractor_never_rejects() {
        let app = Router::new()
            .route(
                "/any",
                get(|OptionalSessionCtx(ctx): OptionalSessionCtx| async move {
                    ctx.is_some().to_string()
                }),
            )
            .layer(session_layer(auth(InMemorySessionStore::default())));

        let (status, response) = drive(app, req_get("/any", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "false");
    }

    #[tokio::test]
    async fn unknown_session_is_401_and_cookie_cleared() {
        let app = me_app(InMemorySessionStore::default());
        let (status, response) = drive(app, req_get("/me", Some("sid=nope"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let set_cookie = set_cookies(&response);
        assert!(set_cookie.contains("Max-Age=0"), "set-cookie: {set_cookie}");
    }

    #[tokio::test]
    async fn handler_set_cookie_is_not_clobbered_by_renewal() {
        let now = SystemTime::now();
        let store = InMemorySessionStore::default();
        // Old-but-valid session that would trigger renewal (past half TTL).
        store
            .create(
                "s1",
                7,
                now + Duration::from_secs(59),
                now - Duration::from_secs(61),
            )
            .await
            .expect("create");
        let auth = auth(store);
        let handler_auth = auth.clone();
        let app = Router::new()
            .route(
                "/login",
                get(move |SessionCtx { user_id }: SessionCtx| {
                    let value = handler_auth.set_cookie_value("s2");
                    async move { ([(header::SET_COOKIE, value)], format!("ok {user_id}")) }
                }),
            )
            .layer(session_layer(auth));

        let (status, response) = drive(app, req_get("/login", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::OK);
        let set_cookie = set_cookies(&response);
        assert_eq!(body_text(response).await, "ok 7");
        assert!(
            !set_cookie.contains("s1"),
            "stale session id clobbered the handler cookie: {set_cookie}"
        );
        assert!(
            set_cookie.contains("sid=s2"),
            "handler cookie missing: {set_cookie}"
        );
        assert!(
            !set_cookie.contains("Max-Age=0"),
            "clear cookie clobbered the handler cookie: {set_cookie}"
        );
    }

    #[derive(Clone)]
    struct FailingStore;

    impl SessionStore for FailingStore {
        async fn create(
            &self,
            _session_id: &str,
            _user_id: i64,
            _expires_at: SystemTime,
            _last_activity: SystemTime,
        ) -> Result<(), ApiError> {
            Ok(())
        }

        async fn find(&self, _session_id: &str) -> Result<Option<SessionRecord>, ApiError> {
            Err(ApiError::internal("lookup failed"))
        }

        async fn touch(
            &self,
            _session_id: &str,
            _expires_at: SystemTime,
            _last_activity: SystemTime,
        ) -> Result<(), ApiError> {
            Ok(())
        }

        async fn remove(&self, _session_id: &str) -> Result<(), ApiError> {
            Ok(())
        }

        async fn remove_by_user(&self, _user_id: i64) -> Result<(), ApiError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn lookup_error_does_not_clear_cookie() {
        let app = Router::new()
            .route(
                "/me",
                get(|SessionCtx { user_id }: SessionCtx| async move { user_id.to_string() }),
            )
            .layer(session_layer(auth(FailingStore)));

        let (status, response) = drive(app, req_get("/me", Some("sid=s1"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let set_cookie = set_cookies(&response);
        assert!(
            !set_cookie.contains("Max-Age=0"),
            "transient lookup error must not clear the client cookie: {set_cookie}"
        );
    }
}
