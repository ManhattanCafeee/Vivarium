//! Extractors that deserialize, initialize, then validate in a single pipeline.
//!
//! The [`Varser`] family wraps axum's native extractors and adds two steps on
//! top of deserialization: [`Initializer`] (a post-deserialization hook) and
//! [`garde::Validate`]. For extraction without these extra steps, use the
//! plain axum [`axum::Json`], [`Query`],
//! [`Path`] or [`Form`] extractors.
//!
//! Validation uses garde's default context: the associated
//! [`garde::Validate::Context`] type must implement `Default` (derived types
//! always do; custom `#[garde(context(...))]` types are not supported).

use axum::body::Bytes;
use axum::extract::{Form, FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::HeaderMap;
use axum::http::header;
use garde::Validate;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// A hook run after deserialization and before validation.
///
/// This mirrors the "initialize" step from the Go reference: use it to trim
/// strings, normalize casing, fill defaults, or reject inconsistent input.
pub trait Initializer {
    /// Adjust the freshly deserialized value before validation runs.
    ///
    /// The default implementation does nothing; override it for types that
    /// need post-deserialization normalization.
    fn try_initialize(&mut self) -> Result<(), ApiError> {
        Ok(())
    }
}

/// Runs the shared [`Initializer`] + [`Validate`] pipeline tail.
fn initialize_and_validate<T: Validate + Initializer>(mut value: T) -> Result<T, ApiError>
where
    T::Context: Default,
{
    value.try_initialize()?;
    value.validate().map_err(|err| {
        let message = err
            .iter()
            .map(|(path, error)| format!("[{path}]: [{error}]"))
            .collect::<Vec<_>>()
            .join(" ");
        ApiError::Validation(message)
    })?;
    Ok(value)
}

/// A JSON-body extractor that deserializes, initializes, then validates.
///
/// This consumes the request body, so it must be the final extractor argument.
pub struct Varser<T>(pub T);

impl<S, T> FromRequest<S> for Varser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|err| ApiError::BadRequest(format!("failed to read body: {err}")))?;
        let value: T =
            serde_json::from_slice(&bytes).map_err(|err| ApiError::Validation(format!("{err}")))?;
        Ok(Varser(initialize_and_validate(value)?))
    }
}

/// A query-string extractor that deserializes, initializes, then validates.
///
/// Backed by [`Query`] + `serde_urlencoded`.
pub struct QueryVarser<T>(pub T);

impl<S, T> FromRequest<S> for QueryVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, _body) = req.into_parts();
        let Query(value) = Query::<T>::from_request_parts(&mut parts, state)
            .await
            .map_err(|err| ApiError::Validation(format!("{err}")))?;
        Ok(QueryVarser(initialize_and_validate(value)?))
    }
}

/// A path-parameter extractor that deserializes, initializes, then validates.
///
/// Backed by [`Path`], deserializing path parameters via
/// `serde`.
pub struct PathVarser<T>(pub T);

impl<S, T> FromRequest<S> for PathVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, _body) = req.into_parts();
        let Path(value) = Path::<T>::from_request_parts(&mut parts, state)
            .await
            .map_err(|err| ApiError::Validation(format!("{err}")))?;
        Ok(PathVarser(initialize_and_validate(value)?))
    }
}

/// A form-body extractor that deserializes, initializes, then validates.
///
/// Backed by [`Form`]. This consumes the request body, so
/// it must be the final extractor argument.
pub struct FormVarser<T>(pub T);

impl<S, T> FromRequest<S> for FormVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Form(value) = Form::<T>::from_request(req, state)
            .await
            .map_err(|err| ApiError::Validation(format!("{err}")))?;
        Ok(FormVarser(initialize_and_validate(value)?))
    }
}

/// Look up the `Authorization` header value, case-insensitively.
///
/// Returns `None` when the header is absent or contains a non-visible-ASCII
/// value.
pub fn get_authorization(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
}

#[cfg(test)]
mod tests {
    use super::{FormVarser, Initializer, PathVarser, QueryVarser, Varser, get_authorization};
    use crate::error::ApiError;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode, header};
    use axum::routing::{get, post};
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use tower::ServiceExt;

    async fn drive(app: Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.oneshot(req).await.expect("request succeeds");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    #[derive(Debug, Deserialize, garde::Validate)]
    struct Credentials {
        #[garde(required, email)]
        email: Option<String>,
    }

    impl Initializer for Credentials {}

    #[tokio::test]
    async fn body_varser_accepts_valid_and_rejects_wrong_type_and_invalid() {
        let app = Router::new().route(
            "/",
            post(
                |Varser(creds): Varser<Credentials>| async move { creds.email.unwrap_or_default() },
            ),
        );

        // Valid body reaches the handler.
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"email":"a@b.com"}"#))
            .unwrap();
        let (status, _) = drive(app.clone(), req).await;
        assert_eq!(status, StatusCode::OK);

        // Wrong type (number where a string is expected) → 422 with a message.
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"email":42}"#))
            .unwrap();
        let (status, body) = drive(app.clone(), req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["message"].as_str().unwrap_or("").contains("string"),
            "serde error should name the expected type: {body}"
        );

        // Validator failure (missing required email) → 422 with field + rule.
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{}"#))
            .unwrap();
        let (status, body) = drive(app, req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let message = body["message"].as_str().expect("message is a string");
        assert!(
            message.starts_with("[email]: [") && message.ends_with(']'),
            "message should be Go-style `[field]: [rule]`: {message}"
        );
    }

    #[derive(Debug, Deserialize, garde::Validate)]
    struct InitRejected {
        #[garde(length(min = 1))]
        value: String,
    }

    impl Initializer for InitRejected {
        fn try_initialize(&mut self) -> Result<(), ApiError> {
            Err(ApiError::Validation("init rejected".to_string()))
        }
    }

    #[tokio::test]
    async fn initializer_error_rejects_with_422() {
        let app = Router::new().route(
            "/",
            post(|Varser(v): Varser<InitRejected>| async move { v.value }),
        );

        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"value":"x"}"#))
            .unwrap();
        let (status, body) = drive(app, req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["message"], "init rejected");
    }

    #[derive(Debug, Deserialize, garde::Validate)]
    struct Search {
        #[garde(length(min = 1))]
        q: String,
    }

    impl Initializer for Search {}

    #[tokio::test]
    async fn query_varser_success_and_failure() {
        let app = Router::new().route(
            "/search",
            get(|QueryVarser(search): QueryVarser<Search>| async move { search.q }),
        );

        let req = Request::builder()
            .uri("/search?q=hello")
            .body(Body::empty())
            .unwrap();
        let (status, _) = drive(app.clone(), req).await;
        assert_eq!(status, StatusCode::OK);

        let req = Request::builder()
            .uri("/search?q=")
            .body(Body::empty())
            .unwrap();
        let (status, body) = drive(app, req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["message"].as_str().unwrap_or("").contains("q"));
    }

    #[derive(Debug, Deserialize, garde::Validate)]
    struct IdParam {
        #[garde(range(min = 0))]
        id: u32,
    }

    impl Initializer for IdParam {}

    #[tokio::test]
    async fn path_varser_success_and_failure() {
        let app = Router::new().route(
            "/x/{id}",
            get(|PathVarser(param): PathVarser<IdParam>| async move { param.id.to_string() }),
        );

        let req = Request::builder().uri("/x/42").body(Body::empty()).unwrap();
        let (status, _) = drive(app.clone(), req).await;
        assert_eq!(status, StatusCode::OK);

        let req = Request::builder()
            .uri("/x/abc")
            .body(Body::empty())
            .unwrap();
        let (status, _) = drive(app, req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[derive(Debug, Deserialize, garde::Validate)]
    struct FormBody {
        #[garde(length(min = 1))]
        name: String,
    }

    impl Initializer for FormBody {}

    #[tokio::test]
    async fn form_varser_success_and_failure() {
        let app = Router::new().route(
            "/form",
            post(|FormVarser(form): FormVarser<FormBody>| async move { form.name }),
        );

        let req = Request::builder()
            .method("POST")
            .uri("/form")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("name=foo"))
            .unwrap();
        let (status, _) = drive(app.clone(), req).await;
        assert_eq!(status, StatusCode::OK);

        let req = Request::builder()
            .method("POST")
            .uri("/form")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("name="))
            .unwrap();
        let (status, body) = drive(app, req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["message"].as_str().unwrap_or("").contains("name"));
    }

    #[test]
    fn get_authorization_is_case_insensitive() {
        for name in ["Authorization", "authorization", "AUTHORIZATION"] {
            let mut headers = HeaderMap::new();
            headers.insert(name, "Bearer tok".parse().unwrap());
            assert_eq!(get_authorization(&headers), Some("Bearer tok"));
        }

        let headers = HeaderMap::new();
        assert_eq!(get_authorization(&headers), None);
    }
}
