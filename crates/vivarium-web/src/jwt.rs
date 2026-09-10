//! JWT signing, verification, and an axum authentication middleware.

use std::future::Future;
use std::pin::Pin;

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

/// Sign `claims` into a JWT using HS256 and the given shared secret.
pub fn sign_token<T: Serialize>(claims: &T, secret: &str) -> Result<String, ApiError> {
    let key = EncodingKey::from_secret(secret.as_bytes());
    encode(&Header::new(Algorithm::HS256), claims, &key)
        .map_err(|err| ApiError::bad_request(format!("failed to sign token: {err}")))
}

/// Decode and validate `token` into claims of type `T`.
///
/// Validation is pinned to HS256: a token whose header declares any other
/// algorithm (for example RS256) is rejected, preventing algorithm-confusion
/// attacks.
///
/// A rejected token is a 401 whose message is the catalog's
/// [`unauthorized`](crate::texts::Texts::unauthorized) text; the reason stays
/// in the error source, where it is logged.
pub fn decode_token<T: DeserializeOwned>(token: &str, secret: &str) -> Result<T, ApiError> {
    let key = DecodingKey::from_secret(secret.as_bytes());
    let validation = Validation::new(Algorithm::HS256);
    decode::<T>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|err| ApiError::unauthorized(texts().unauthorized.clone()).with_source(err))
}

/// The boxed future returned by the JWT authentication middleware.
pub type JwtAuthFuture = Pin<Box<dyn Future<Output = Result<Response, ApiError>> + Send + 'static>>;

/// The layer returned by [`jwt_auth`].
pub type JwtAuthLayer = FromFnLayer<
    fn(State<String>, Request, Next) -> JwtAuthFuture,
    String,
    (State<String>, Request),
>;

/// Extracts the bearer token from the `Authorization` header, if present.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let (scheme, token) = get_authorization(headers)?.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        Some(token)
    } else {
        None
    }
}

/// The middleware function backing [`jwt_auth`].
fn jwt_middleware<T>(State(secret): State<String>, mut req: Request, next: Next) -> JwtAuthFuture
where
    T: DeserializeOwned + Send + Sync + Clone + 'static,
{
    Box::pin(async move {
        let token = bearer_token(req.headers())
            .ok_or_else(|| ApiError::unauthorized(texts().unauthorized.clone()))?;
        let claims: T = decode_token(token, &secret)?;
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
    middleware::from_fn_with_state(
        secret,
        jwt_middleware::<T> as fn(State<String>, Request, Next) -> JwtAuthFuture,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Extension;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::routing::get;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use http_body_util::BodyExt;
    use jsonwebtoken::get_current_timestamp;
    use serde::{Deserialize, Serialize};
    use tower::ServiceExt;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct Claims {
        sub: String,
        exp: u64,
    }

    const SECRET: &str = "test-secret";

    fn fresh_claims(sub: &str) -> Claims {
        Claims {
            sub: sub.to_string(),
            exp: get_current_timestamp() + 3600,
        }
    }

    #[tokio::test]
    async fn sign_decode_roundtrip() {
        let claims = fresh_claims("alice");
        let token = sign_token(&claims, SECRET).expect("sign token");
        let decoded: Claims = decode_token(&token, SECRET).expect("decode token");
        assert_eq!(decoded, claims);
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let claims = Claims {
            sub: "bob".to_string(),
            exp: get_current_timestamp() - 3600,
        };
        let token = sign_token(&claims, SECRET).expect("sign token");
        let err = decode_token::<Claims>(&token, SECRET).expect_err("must reject expired token");
        assert_eq!(err.kind(), crate::error::ErrorKind::Unauthorized);
    }

    #[tokio::test]
    async fn rs256_token_is_rejected() {
        // Craft a token whose header declares RS256 while still signing the
        // payload with an HMAC shared secret. HS256-only validation must
        // refuse it on the algorithm mismatch before trusting the signature.
        let claims = fresh_claims("carol");
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("serialize claims"));
        let signing_input = format!("{header}.{payload}");
        let signature = jsonwebtoken::crypto::sign(
            signing_input.as_bytes(),
            &EncodingKey::from_secret(SECRET.as_bytes()),
            Algorithm::HS256,
        )
        .expect("sign signing input");
        let token = format!("{signing_input}.{signature}");

        let err = decode_token::<Claims>(&token, SECRET).expect_err("must reject RS256 token");
        assert_eq!(err.kind(), crate::error::ErrorKind::Unauthorized);
    }

    #[tokio::test]
    async fn jwt_auth_middleware_accepts_and_rejects() {
        let app = Router::new()
            .route(
                "/me",
                get(|Extension(claims): Extension<Claims>| async move { claims.sub }),
            )
            .layer(jwt_auth::<Claims>(SECRET.to_string()));

        // Valid token reaches the handler with claims in extensions.
        let token = sign_token(&fresh_claims("alice"), SECRET).expect("sign token");
        let req = Request::builder()
            .uri("/me")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(req).await.expect("request succeeds");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        assert_eq!(&bytes[..], b"alice");

        // Missing token → 401 with the numeric code.
        let req = Request::builder().uri("/me").body(Body::empty()).unwrap();
        let response = app.clone().oneshot(req).await.expect("request succeeds");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("error body is json");
        assert_eq!(body["code"], 401);
        assert_eq!(body["message"], crate::texts::texts().unauthorized.as_ref());

        // Wrong secret / invalid token → 401.
        let bad = sign_token(&fresh_claims("mallory"), "wrong-secret").expect("sign token");
        let req = Request::builder()
            .uri("/me")
            .header(header::AUTHORIZATION, format!("Bearer {bad}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.expect("request succeeds");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
