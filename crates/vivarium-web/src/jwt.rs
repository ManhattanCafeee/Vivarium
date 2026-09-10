//! JWT signing, verification, and axum authentication middlewares.
//!
//! HS256 is the only accepted algorithm: a token whose header declares
//! anything else (for example RS256) is rejected, which closes the
//! algorithm-confusion attack. The signing secret is the configured primary
//! key; [`KeyRing::previous`] holds retired secrets that still verify during a
//! rotation window.
//!
//! Two layers build on a [`JwtVerifier`]:
//!
//! - [`JwtVerifier::layer`] requires a bearer token and stores the decoded
//!   claims in the request extensions, where [`Extension`](axum::Extension)
//!   extracts them.
//! - [`JwtVerifier::optional_layer`] stores `Option<C>` instead: a request
//!   without an `Authorization` header passes through as `None`, while a
//!   present-but-invalid token is still rejected.
//!
//! ```no_run
//! use axum::{Extension, Router, routing::get};
//! use serde::{Deserialize, Serialize};
//! use vivarium_web::jwt::{JwtConfig, JwtVerifier, KeyRing};
//!
//! #[derive(Clone, Serialize, Deserialize)]
//! struct Claims {
//!     sub: u64,
//!     exp: i64,
//! }
//!
//! let verifier = JwtVerifier::new(
//!     JwtConfig::default(),
//!     KeyRing::new("current-secret"),
//! );
//! let token = verifier.encode(&Claims { sub: 7, exp: 4_000_000_000 }).expect("sign");
//! let claims: Claims = verifier.decode(&token).expect("verify");
//! assert_eq!(claims.sub, 7);
//!
//! let app: Router = Router::new()
//!     .route("/me", get(|Extension(claims): Extension<Claims>| async move {
//!         claims.sub.to_string()
//!     }))
//!     .layer(verifier.layer::<Claims>());
//! ```

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::{self, FromFnLayer, Next};
use axum::response::Response;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ApiError;
use crate::texts::texts;
use crate::varser::get_authorization;

/// The largest clock skew a [`JwtConfig`] may ask for.
///
/// jsonwebtoken compares expiry as `now - leeway` on `u64` without saturating,
/// so an unbounded value would overflow; a day is already far beyond any real
/// skew (and effectively means "ignore `exp`").
const MAX_LEEWAY: Duration = Duration::from_secs(24 * 3600);

/// How a [`JwtVerifier`] validates a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtConfig {
    /// Clock skew tolerated on `exp` and `nbf`, in seconds, capped at 24 hours
    /// (a larger value is clamped, since it would overflow jsonwebtoken's own
    /// expiry arithmetic).
    pub leeway: Duration,
    /// The `aud` every token must carry and match.
    ///
    /// `None` disables the audience check entirely, so tokens carrying any (or
    /// no) `aud` verify. `Some` both requires the claim to be present and
    /// compares it, so a token minted for another audience — or one that omits
    /// `aud` altogether — is rejected.
    pub audience: Option<String>,
    /// The `iss` every token must carry and match, with the same semantics as
    /// [`audience`](Self::audience): `Some` makes the claim required.
    pub issuer: Option<String>,
    /// Registered claims that must be present, on top of `exp` and of any
    /// `aud`/`iss` required through the fields above. Defaults to `["exp"]`.
    pub required_spec_claims: Vec<String>,
}

impl Default for JwtConfig {
    /// The library defaults: 60 seconds of leeway, no audience or issuer
    /// requirement, and `exp` mandatory.
    fn default() -> Self {
        Self {
            leeway: Duration::from_secs(60),
            audience: None,
            issuer: None,
            required_spec_claims: vec!["exp".to_string()],
        }
    }
}

/// The signing keys of a deployment: one primary, plus retired ones.
///
/// [`encode`](JwtVerifier::encode) always signs with `primary`; a token is
/// verified against `primary` first and then against each of `previous` in
/// order, which is what lets a rotation overlap (old tokens keep verifying
/// until they expire, new ones are only accepted under the new secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRing {
    /// The secret new tokens are signed with.
    pub primary: String,
    /// Retired secrets that still verify, newest first.
    pub previous: Vec<String>,
}

impl KeyRing {
    /// A ring with a single signing key.
    pub fn new(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            previous: Vec::new(),
        }
    }

    /// The secrets to try when verifying, in order.
    fn verification_keys(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.primary.as_str()).chain(self.previous.iter().map(String::as_str))
    }
}

/// Signs and verifies tokens with one algorithm, one configuration and a key
/// ring.
///
/// ```
/// use vivarium_web::jwt::{JwtConfig, JwtVerifier, KeyRing};
///
/// let verifier = JwtVerifier::new(JwtConfig::default(), KeyRing::new("secret"));
/// let token = verifier.encode(&serde_json::json!({ "sub": 1, "exp": 4_000_000_000_i64 }))
///     .expect("sign");
/// let claims: serde_json::Value = verifier.decode(&token).expect("verify");
/// assert_eq!(claims["sub"], 1);
/// ```
#[derive(Debug, Clone)]
pub struct JwtVerifier {
    config: JwtConfig,
    keys: KeyRing,
}

impl JwtVerifier {
    /// Builds a verifier from a configuration and a key ring.
    pub fn new(config: JwtConfig, keys: KeyRing) -> Self {
        Self { config, keys }
    }

    /// The validation configuration.
    pub fn config(&self) -> &JwtConfig {
        &self.config
    }

    /// The signing keys.
    pub fn keys(&self) -> &KeyRing {
        &self.keys
    }

    /// Signs `claims`, always with the primary key.
    ///
    /// A signing failure is internal (the claims could not be serialized, or
    /// the key was unusable); its detail stays in the error source for the
    /// logs, never in the client's `message`.
    pub fn encode<C: Serialize>(&self, claims: &C) -> Result<String, ApiError> {
        let key = EncodingKey::from_secret(self.keys.primary.as_bytes());
        encode(&Header::new(Algorithm::HS256), claims, &key).map_err(ApiError::internal)
    }

    /// Verifies `token` and returns its claims.
    ///
    /// The signature is checked against the primary key and then against the
    /// retired ones, and the claims against the [`JwtConfig`]. Any rejection is
    /// a 401 whose message is the catalog's
    /// [`unauthorized`](crate::texts::Texts::unauthorized) text; the reason
    /// stays in the error source, where it is logged.
    pub fn decode<C: DeserializeOwned>(&self, token: &str) -> Result<C, ApiError> {
        let validation = self.validation();
        let mut failure = None;
        for secret in self.keys.verification_keys() {
            let key = DecodingKey::from_secret(secret.as_bytes());
            match decode::<C>(token, &key, &validation) {
                Ok(data) => return Ok(data.claims),
                Err(err) => {
                    failure.get_or_insert(err);
                }
            }
        }
        let unauthorized = ApiError::unauthorized(texts().unauthorized.clone());
        match failure {
            Some(err) => Err(unauthorized.with_source(err)),
            // Unreachable: a key ring always holds at least the primary key.
            None => Err(unauthorized),
        }
    }

    /// An axum layer requiring a valid bearer token.
    ///
    /// Claims are inserted into the request extensions, so a handler reads
    /// them with `Extension<C>`. Missing or invalid tokens are 401s.
    pub fn layer<C>(&self) -> JwtAuthLayer
    where
        C: DeserializeOwned + Send + Sync + Clone + 'static,
    {
        middleware::from_fn_with_state(
            self.clone(),
            jwt_middleware::<C> as fn(State<JwtVerifier>, Request, Next) -> JwtAuthFuture,
        )
    }

    /// An axum layer that authenticates when a bearer token is present.
    ///
    /// The extensions receive `Option<C>`: handlers read them with
    /// `Extension<Option<C>>` and get `None` when the request carried no
    /// `Authorization: Bearer …` header. A token that *is* present must be
    /// valid — a malformed or expired one is a 401, never silently `None`.
    pub fn optional_layer<C>(&self) -> OptionalJwtAuthLayer
    where
        C: DeserializeOwned + Send + Sync + Clone + 'static,
    {
        middleware::from_fn_with_state(
            self.clone(),
            optional_jwt_middleware::<C> as fn(State<JwtVerifier>, Request, Next) -> JwtAuthFuture,
        )
    }

    /// The jsonwebtoken validation built from the configuration.
    fn validation(&self) -> Validation {
        let mut validation = Validation::new(Algorithm::HS256);
        // jsonwebtoken's own arithmetic is not saturating, so an absurd leeway
        // would overflow its expiry comparison in a debug build (and turn every
        // token into an expired one in a release build). A day is already far
        // beyond any real clock skew.
        validation.leeway = self.config.leeway.as_secs().min(MAX_LEEWAY.as_secs());
        // `nbf` carries the "not before" semantics of RFC 7519; the default
        // jsonwebtoken validation leaves it unenforced.
        validation.validate_nbf = true;
        validation.required_spec_claims = self
            .config
            .required_spec_claims
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        match &self.config.audience {
            Some(audience) => {
                validation.set_audience(std::slice::from_ref(audience));
                // A configured audience means the claim must be there at all:
                // jsonwebtoken only compares a claim that is present, so
                // without this a token that simply omits `aud` would pass.
                validation.required_spec_claims.insert("aud".to_string());
            }
            // jsonwebtoken rejects any token carrying an `aud` when an audience
            // is configured but none was expected; leave `aud` unchecked when
            // this deployment does not scope tokens by audience.
            None => validation.validate_aud = false,
        }
        if let Some(issuer) = &self.config.issuer {
            validation.set_issuer(std::slice::from_ref(issuer));
            validation.required_spec_claims.insert("iss".to_string());
        }
        validation
    }
}

/// Sign `claims` into a JWT using HS256 and the given shared secret.
///
/// Shorthand for [`JwtVerifier::encode`] with the default configuration and a
/// single key; build a [`JwtVerifier`] when you need leeway, audience, issuer
/// or key rotation.
pub fn sign_token<T: Serialize>(claims: &T, secret: &str) -> Result<String, ApiError> {
    JwtVerifier::new(JwtConfig::default(), KeyRing::new(secret)).encode(claims)
}

/// Decode and validate `token` into claims of type `T`.
///
/// Shorthand for [`JwtVerifier::decode`] with the default configuration and a
/// single key. A rejected token is a 401 whose message is the catalog's
/// [`unauthorized`](crate::texts::Texts::unauthorized) text; the reason stays
/// in the error source, where it is logged.
pub fn decode_token<T: DeserializeOwned>(token: &str, secret: &str) -> Result<T, ApiError> {
    JwtVerifier::new(JwtConfig::default(), KeyRing::new(secret)).decode(token)
}

/// The boxed future returned by the JWT authentication middlewares.
pub type JwtAuthFuture = Pin<Box<dyn Future<Output = Result<Response, ApiError>> + Send + 'static>>;

/// The layer returned by [`JwtVerifier::layer`] and [`jwt_auth`].
pub type JwtAuthLayer = FromFnLayer<
    fn(State<JwtVerifier>, Request, Next) -> JwtAuthFuture,
    JwtVerifier,
    (State<JwtVerifier>, Request),
>;

/// The layer returned by [`JwtVerifier::optional_layer`].
///
/// The same middleware shape as [`JwtAuthLayer`], kept as its own name so
/// signatures say which contract a route opts into.
pub type OptionalJwtAuthLayer = JwtAuthLayer;

/// Extracts the bearer token from the `Authorization` header, if present.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let (scheme, token) = get_authorization(headers)?.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        Some(token)
    } else {
        None
    }
}

/// The middleware function backing [`JwtVerifier::layer`].
fn jwt_middleware<C>(
    State(verifier): State<JwtVerifier>,
    mut req: Request,
    next: Next,
) -> JwtAuthFuture
where
    C: DeserializeOwned + Send + Sync + Clone + 'static,
{
    Box::pin(async move {
        let token = bearer_token(req.headers())
            .ok_or_else(|| ApiError::unauthorized(texts().unauthorized.clone()))?;
        let claims: C = verifier.decode(token)?;
        req.extensions_mut().insert(claims);
        Ok(next.run(req).await)
    })
}

/// The middleware function backing [`JwtVerifier::optional_layer`].
fn optional_jwt_middleware<C>(
    State(verifier): State<JwtVerifier>,
    mut req: Request,
    next: Next,
) -> JwtAuthFuture
where
    C: DeserializeOwned + Send + Sync + Clone + 'static,
{
    Box::pin(async move {
        let claims: Option<C> = match bearer_token(req.headers()) {
            Some(token) => Some(verifier.decode(token)?),
            None => None,
        };
        req.extensions_mut().insert(claims);
        Ok(next.run(req).await)
    })
}

/// Build an axum middleware layer that authenticates bearer JWTs.
///
/// The middleware reads the `Authorization: Bearer <token>` header, verifies
/// the HS256 signature, and stores the decoded claims of type `T` in the
/// request extensions. Handlers retrieve them via
/// [`Extension<T>`](axum::Extension):
///
/// ```no_run
/// # use axum::Extension;
/// # use serde::{Deserialize, Serialize};
/// # use vivarium_web::jwt::jwt_auth;
/// # #[derive(Clone, Serialize, Deserialize)]
/// # struct Claims { sub: String }
/// # fn example() {
/// # let secret = "s3cret".to_string();
/// let app: axum::Router = axum::Router::new()
///     .route("/me", axum::routing::get(
///         |Extension(claims): Extension<Claims>| async move { claims.sub }
///     ))
///     .layer(jwt_auth::<Claims>(secret));
/// # }
/// ```
///
/// Failures produce a 401 whose message is the catalog's
/// [`unauthorized`](crate::texts::Texts::unauthorized) text.
pub fn jwt_auth<T>(secret: String) -> JwtAuthLayer
where
    T: DeserializeOwned + Send + Sync + Clone + 'static,
{
    JwtVerifier::new(JwtConfig::default(), KeyRing::new(secret)).layer::<T>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Extension;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use tower::ServiceExt;

    const SECRET: &str = "test-secret";

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestClaims {
        sub: u64,
        exp: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aud: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        iss: Option<String>,
    }

    fn claims(exp: i64) -> TestClaims {
        TestClaims {
            sub: 7,
            exp,
            aud: None,
            iss: None,
        }
    }

    fn future_exp() -> i64 {
        jsonwebtoken::get_current_timestamp() as i64 + 3600
    }

    fn verifier() -> JwtVerifier {
        JwtVerifier::new(JwtConfig::default(), KeyRing::new(SECRET))
    }

    async fn drive(app: Router, req: Request<Body>) -> (StatusCode, Response) {
        let response = app.oneshot(req).await.expect("request succeeds");
        (response.status(), response)
    }

    fn req_with_bearer(token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/me");
        if let Some(token) = token {
            builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::empty()).expect("request builds")
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

    fn claims_app(verifier: JwtVerifier) -> Router {
        Router::new()
            .route(
                "/me",
                get(|Extension(claims): Extension<TestClaims>| async move {
                    claims.sub.to_string()
                }),
            )
            .layer(verifier.layer::<TestClaims>())
    }

    #[test]
    fn round_trip_returns_the_claims() {
        let verifier = verifier();
        let token = verifier.encode(&claims(future_exp())).expect("sign");
        let decoded: TestClaims = verifier.decode(&token).expect("verify");
        assert_eq!(decoded.sub, 7);
    }

    #[test]
    fn foreign_algorithm_is_rejected() {
        // Signed correctly, but with a header that is not HS256: pinning the
        // algorithm in the decoder is what rejects it.
        let key = EncodingKey::from_secret(SECRET.as_bytes());
        let token = encode(&Header::new(Algorithm::HS384), &claims(future_exp()), &key)
            .expect("sign with another algorithm");

        let error = verifier()
            .decode::<TestClaims>(&token)
            .expect_err("must reject");
        assert_eq!(error.kind(), crate::error::ErrorKind::Unauthorized);
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn wrong_secret_is_unauthorized_with_the_catalog_message() {
        let token = sign_token(&claims(future_exp()), "other-secret").expect("sign");
        let error = verifier()
            .decode::<TestClaims>(&token)
            .expect_err("must reject");
        assert_eq!(error.kind(), crate::error::ErrorKind::Unauthorized);
        assert_eq!(error.message(), texts().unauthorized.as_ref());
    }

    #[test]
    fn expiry_is_enforced_and_leeway_is_honoured() {
        let verifier = verifier();
        let now = jsonwebtoken::get_current_timestamp() as i64;

        // Inside the 60s default leeway.
        let recent = verifier.encode(&claims(now - 30)).expect("sign");
        assert!(verifier.decode::<TestClaims>(&recent).is_ok());

        // Well past it.
        let old = verifier.encode(&claims(now - 3600)).expect("sign");
        assert_eq!(
            verifier
                .decode::<TestClaims>(&old)
                .expect_err("expired")
                .kind(),
            crate::error::ErrorKind::Unauthorized
        );

        // No leeway at all.
        let strict = JwtVerifier::new(
            JwtConfig {
                leeway: Duration::ZERO,
                ..JwtConfig::default()
            },
            KeyRing::new(SECRET),
        );
        assert!(strict.decode::<TestClaims>(&recent).is_err());
    }

    #[test]
    fn previous_keys_verify_but_never_sign() {
        let ring = KeyRing {
            primary: "new-secret".to_string(),
            previous: vec!["old-secret".to_string()],
        };
        let verifier = JwtVerifier::new(JwtConfig::default(), ring);
        let claims = claims(future_exp());

        let old_token = sign_token(&claims, "old-secret").expect("sign");
        assert_eq!(
            verifier
                .decode::<TestClaims>(&old_token)
                .expect("verifies")
                .sub,
            7
        );

        let new_token = verifier.encode(&claims).expect("sign");
        assert!(verifier.decode::<TestClaims>(&new_token).is_ok());
        // A ring without the retired key accepts the new token and rejects the
        // old one: `encode` signs with `primary` only.
        let rotated = JwtVerifier::new(JwtConfig::default(), KeyRing::new("new-secret"));
        assert!(
            rotated.decode::<TestClaims>(&new_token).is_ok(),
            "a freshly signed token must verify under the primary key alone"
        );
        assert!(rotated.decode::<TestClaims>(&old_token).is_err());
    }

    #[test]
    fn audience_and_issuer_are_checked_when_configured() {
        let scoped = JwtVerifier::new(
            JwtConfig {
                audience: Some("vivarium-api".to_string()),
                issuer: Some("vivarium-auth".to_string()),
                ..JwtConfig::default()
            },
            KeyRing::new(SECRET),
        );

        let good = scoped
            .encode(&TestClaims {
                aud: Some("vivarium-api".to_string()),
                iss: Some("vivarium-auth".to_string()),
                ..claims(future_exp())
            })
            .expect("sign");
        assert!(scoped.decode::<TestClaims>(&good).is_ok());

        let wrong_audience = scoped
            .encode(&TestClaims {
                aud: Some("someone-else".to_string()),
                iss: Some("vivarium-auth".to_string()),
                ..claims(future_exp())
            })
            .expect("sign");
        assert!(scoped.decode::<TestClaims>(&wrong_audience).is_err());

        let wrong_issuer = scoped
            .encode(&TestClaims {
                aud: Some("vivarium-api".to_string()),
                iss: Some("someone-else".to_string()),
                ..claims(future_exp())
            })
            .expect("sign");
        assert!(scoped.decode::<TestClaims>(&wrong_issuer).is_err());

        // A token that simply omits the required claims must not slip through:
        // jsonwebtoken only compares a claim that is present.
        let without_audience = scoped.encode(&claims(future_exp())).expect("sign");
        assert!(scoped.decode::<TestClaims>(&without_audience).is_err());
    }

    #[test]
    fn not_before_is_enforced_with_the_configured_leeway() {
        let verifier = verifier();
        let now = jsonwebtoken::get_current_timestamp() as i64;
        let token = |nbf: i64| {
            verifier
                .encode(&serde_json::json!({
                    "sub": 7,
                    "exp": now + 3600,
                    "nbf": nbf,
                }))
                .expect("sign")
        };

        assert!(
            verifier.decode::<TestClaims>(&token(now - 10)).is_ok(),
            "a token that is already valid must pass"
        );
        assert!(
            verifier.decode::<TestClaims>(&token(now + 3600)).is_err(),
            "a token that is not valid yet must be rejected"
        );
    }

    #[test]
    fn a_signing_failure_is_an_internal_error_without_the_detail() {
        /// Claims that refuse to serialize, whatever the serializer.
        struct Unserializable;

        impl Serialize for Unserializable {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("the claims cannot be encoded"))
            }
        }

        let error = verifier()
            .encode(&Unserializable)
            .expect_err("claims that cannot be serialized must not sign");
        assert_eq!(error.kind(), crate::error::ErrorKind::Internal);
        assert_eq!(error.message(), texts().internal.as_ref());
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn an_unconfigured_audience_does_not_reject_aud_tokens() {
        // The default config checks no audience, so a token that happens to
        // carry one still verifies.
        let token = verifier()
            .encode(&TestClaims {
                aud: Some("anything".to_string()),
                ..claims(future_exp())
            })
            .expect("sign");
        assert!(verifier().decode::<TestClaims>(&token).is_ok());
    }

    #[test]
    fn required_spec_claims_make_a_claim_mandatory() {
        let demanding = JwtVerifier::new(
            JwtConfig {
                required_spec_claims: vec!["exp".to_string(), "iss".to_string()],
                ..JwtConfig::default()
            },
            KeyRing::new(SECRET),
        );

        let without_iss = demanding.encode(&claims(future_exp())).expect("sign");
        assert!(demanding.decode::<TestClaims>(&without_iss).is_err());

        let with_iss = demanding
            .encode(&TestClaims {
                iss: Some("vivarium-auth".to_string()),
                ..claims(future_exp())
            })
            .expect("sign");
        assert!(demanding.decode::<TestClaims>(&with_iss).is_ok());
    }

    #[tokio::test]
    async fn layer_requires_a_token_and_injects_the_claims() {
        let verifier = verifier();
        let token = verifier.encode(&claims(future_exp())).expect("sign");

        let (status, response) =
            drive(claims_app(verifier.clone()), req_with_bearer(Some(&token))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "7");

        let (status, response) = drive(claims_app(verifier.clone()), req_with_bearer(None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let json: serde_json::Value =
            serde_json::from_str(&body_text(response).await).expect("error body is json");
        assert_eq!(json["code"], 401);
        assert_eq!(json["message"], texts().unauthorized.as_ref());

        let (status, _) = drive(claims_app(verifier), req_with_bearer(Some("garbage"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn jwt_auth_free_function_still_authenticates() {
        let token = sign_token(&claims(future_exp()), SECRET).expect("sign");
        let app =
            Router::new()
                .route(
                    "/me",
                    get(|Extension(claims): Extension<TestClaims>| async move {
                        claims.sub.to_string()
                    }),
                )
                .layer(jwt_auth::<TestClaims>(SECRET.to_string()));

        let (status, response) = drive(app, req_with_bearer(Some(&token))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "7");
    }

    #[tokio::test]
    async fn optional_layer_distinguishes_absent_from_invalid() {
        let verifier = verifier();
        let token = verifier.encode(&claims(future_exp())).expect("sign");
        let app = || {
            Router::new()
                .route(
                    "/me",
                    get(
                        |Extension(claims): Extension<Option<TestClaims>>| async move {
                            match claims {
                                Some(claims) => claims.sub.to_string(),
                                None => "anonymous".to_string(),
                            }
                        },
                    ),
                )
                .layer(verifier.clone().optional_layer::<TestClaims>())
        };

        let (status, response) = drive(app(), req_with_bearer(None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "anonymous");

        let (status, response) = drive(app(), req_with_bearer(Some(&token))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_text(response).await, "7");

        // A present but invalid token is a failure, not an anonymous request.
        let (status, _) = drive(app(), req_with_bearer(Some("garbage"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
