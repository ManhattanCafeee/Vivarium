//! Extractors that deserialize, initialize, then validate in one pipeline.
//!
//! The `Varser` family wraps axum's native extractors and adds two steps on top
//! of deserialization: [`Initializer`] (a post-deserialization hook) and
//! validation. With the default `validation-validator` feature the canonical
//! extractors — `Varser`, `QueryVarser`, `PathVarser`, `FormVarser` — bind
//! `validator::Validate`; the `validation-garde` feature adds the `Garde*`
//! equivalents binding `garde::Validate`. For extraction without
//! these steps use axum's [`axum::Json`], [`axum::extract::Query`],
//! [`axum::extract::Path`] or [`axum::extract::Form`].
//!
//! Rejections follow the crate's status-code contract:
//!
//! | Failure | Status | Kind |
//! |---|---|---|
//! | the body could not be read | 400 | [`BadRequest`](crate::ErrorKind::BadRequest) |
//! | the body is not JSON, or does not match the type | 400 | [`DataParse`](crate::ErrorKind::DataParse) |
//! | a path, query or form parameter failed to parse | 400 | [`BadRequest`](crate::ErrorKind::BadRequest) |
//! | [`Initializer`] rejected the value | whatever it returned | — |
//! | a validation rule failed | 422 | [`Validation`](crate::ErrorKind::Validation), with `errors` |
//!
//! The text of a parse rejection is the catalog's
//! [`bad_request`](crate::texts::Texts::bad_request) /
//! [`data_parse`](crate::texts::Texts::data_parse) message unless
//! [`Texts::echo_details`](crate::texts::Texts::echo_details) is enabled, in
//! which case the upstream detail is repeated verbatim.

#[cfg(any(feature = "validation-validator", feature = "validation-garde"))]
use axum::body::Bytes;
#[cfg(any(feature = "validation-validator", feature = "validation-garde"))]
use axum::extract::{Form, FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::HeaderMap;
use axum::http::header;
#[cfg(any(feature = "validation-validator", feature = "validation-garde"))]
use serde::de::DeserializeOwned;

use crate::error::ApiError;
#[cfg(any(feature = "validation-validator", feature = "validation-garde"))]
use crate::error::{ErrorKind, detail};

/// A hook run after deserialization and before validation.
///
/// This mirrors the "initialize" step from the Go reference: use it to trim
/// strings, normalize casing, fill defaults, or reject inconsistent input.
/// Implement it for every type used with the `Varser` family; the default
/// implementation does nothing.
pub trait Initializer {
    /// Adjust the freshly deserialized value before validation runs.
    ///
    /// Returning an error aborts extraction with that error, so a rejection
    /// here reaches the client as the [`ApiError`] it is.
    fn try_initialize(&mut self) -> Result<(), ApiError> {
        Ok(())
    }
}

/// Deserializes a JSON body, then runs the shared pipeline tail.
#[cfg(feature = "validation-validator")]
pub struct Varser<T>(pub T);

#[cfg(feature = "validation-validator")]
impl<S, T> FromRequest<S> for Varser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + validator::Validate + Initializer + Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        let value: T = serde_json::from_slice(&bytes)?;
        Ok(Self(initialize_and_validate(value)?))
    }
}

/// Deserializes a query string, then runs the shared pipeline tail.
#[cfg(feature = "validation-validator")]
pub struct QueryVarser<T>(pub T);

#[cfg(feature = "validation-validator")]
impl<S, T> FromRequest<S> for QueryVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + validator::Validate + Initializer + Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, _body) = req.into_parts();
        let Query(value) = Query::<T>::from_request_parts(&mut parts, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        Ok(Self(initialize_and_validate(value)?))
    }
}

/// Deserializes path parameters, then runs the shared pipeline tail.
#[cfg(feature = "validation-validator")]
pub struct PathVarser<T>(pub T);

#[cfg(feature = "validation-validator")]
impl<S, T> FromRequest<S> for PathVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + validator::Validate + Initializer + Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, _body) = req.into_parts();
        let Path(value) = Path::<T>::from_request_parts(&mut parts, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        Ok(Self(initialize_and_validate(value)?))
    }
}

/// Deserializes an urlencoded form body, then runs the shared pipeline tail.
#[cfg(feature = "validation-validator")]
pub struct FormVarser<T>(pub T);

#[cfg(feature = "validation-validator")]
impl<S, T> FromRequest<S> for FormVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + validator::Validate + Initializer + Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Form(value) = Form::<T>::from_request(req, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        Ok(Self(initialize_and_validate(value)?))
    }
}

/// [`Initializer`] + [`validator::Validate`], the canonical pipeline tail.
#[cfg(feature = "validation-validator")]
fn initialize_and_validate<T>(mut value: T) -> Result<T, ApiError>
where
    T: validator::Validate + Initializer,
{
    value.try_initialize()?;
    value
        .validate()
        .map_err(|errors| ApiError::validation(errors.into()))?;
    Ok(value)
}

/// Deserializes a JSON body, then runs the garde pipeline tail.
#[cfg(feature = "validation-garde")]
pub struct GardeVarser<T>(pub T);

#[cfg(feature = "validation-garde")]
impl<S, T> FromRequest<S> for GardeVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + garde::Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        let value: T = serde_json::from_slice(&bytes)?;
        Ok(Self(initialize_and_validate_garde(value)?))
    }
}

/// Deserializes a query string, then runs the garde pipeline tail.
#[cfg(feature = "validation-garde")]
pub struct GardeQueryVarser<T>(pub T);

#[cfg(feature = "validation-garde")]
impl<S, T> FromRequest<S> for GardeQueryVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + garde::Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, _body) = req.into_parts();
        let Query(value) = Query::<T>::from_request_parts(&mut parts, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        Ok(Self(initialize_and_validate_garde(value)?))
    }
}

/// Deserializes path parameters, then runs the garde pipeline tail.
#[cfg(feature = "validation-garde")]
pub struct GardePathVarser<T>(pub T);

#[cfg(feature = "validation-garde")]
impl<S, T> FromRequest<S> for GardePathVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + garde::Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, _body) = req.into_parts();
        let Path(value) = Path::<T>::from_request_parts(&mut parts, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        Ok(Self(initialize_and_validate_garde(value)?))
    }
}

/// Deserializes an urlencoded form body, then runs the garde pipeline tail.
#[cfg(feature = "validation-garde")]
pub struct GardeFormVarser<T>(pub T);

#[cfg(feature = "validation-garde")]
impl<S, T> FromRequest<S> for GardeFormVarser<T>
where
    S: Send + Sync,
    T: DeserializeOwned + garde::Validate + Initializer + Send + Sync,
    T::Context: Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Form(value) = Form::<T>::from_request(req, state)
            .await
            .map_err(|error| detail(ErrorKind::BadRequest, error.to_string()))?;
        Ok(Self(initialize_and_validate_garde(value)?))
    }
}

/// [`Initializer`] + [`garde::Validate`], the optional pipeline tail.
///
/// `garde` validates against an associated `Context`, which must be
/// `Default`; derived types always are.
#[cfg(feature = "validation-garde")]
fn initialize_and_validate_garde<T>(mut value: T) -> Result<T, ApiError>
where
    T: garde::Validate + Initializer,
    T::Context: Default,
{
    value.try_initialize()?;
    value
        .validate()
        .map_err(|report| ApiError::validation(report.into()))?;
    Ok(value)
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

#[cfg(all(test, feature = "validation-validator"))]
mod validator_tests {
    use super::{FormVarser, Initializer, PathVarser, QueryVarser, Varser, get_authorization};
    use crate::error::ApiError;
    use crate::texts::texts;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode, header};
    use axum::routing::{get, post};
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use tower::ServiceExt;
    use validator::Validate;

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

    #[derive(Debug, Deserialize, Validate)]
    struct Credentials {
        #[validate(length(min = 3, message = "too short"))]
        email: String,
    }

    impl Initializer for Credentials {}

    fn credentials_app() -> Router {
        Router::new().route(
            "/",
            post(|Varser(creds): Varser<Credentials>| async move { axum::Json(creds.email) }),
        )
    }

    fn json_request(body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// A valid body reaches the handler.
    #[tokio::test]
    async fn body_extractor_accepts_and_initializes() {
        let (status, body) =
            drive(credentials_app(), json_request(r#"{"email":"ada@x.io"}"#)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::Value::String("ada@x.io".to_string()));
    }

    /// A type mismatch is a *syntax* failure: 400 `DataParse`, no `errors`.
    #[tokio::test]
    async fn body_type_mismatch_is_400_data_parse() {
        let (status, body) = drive(credentials_app(), json_request(r#"{"email":42}"#)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], 400);
        assert_eq!(body["message"], texts().data_parse.as_ref());
        assert!(
            body.get("errors").is_none(),
            "parse failures carry no errors: {body}"
        );
        assert_eq!(body["data"], serde_json::Value::Null);
    }

    /// Malformed JSON is a 400 too, not the 422 it used to be.
    #[tokio::test]
    async fn malformed_json_is_400_data_parse() {
        let (status, body) = drive(credentials_app(), json_request("{")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], 400);
        assert_eq!(body["message"], texts().data_parse.as_ref());
    }

    /// A failed rule is a 422 carrying the flattened structured payload.
    #[tokio::test]
    async fn validation_failure_is_422_with_errors() {
        let (status, body) = drive(credentials_app(), json_request(r#"{"email":"a"}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], 422);
        assert_eq!(body["message"], texts().validation.as_ref());
        assert_eq!(body["errors"]["email"][0]["code"], "length");
        assert_eq!(body["errors"]["email"][0]["message"], "too short");
        assert_eq!(body["errors"]["email"][0]["params"]["min"], 3);
    }

    #[derive(Debug, Deserialize, Validate)]
    struct InitRejected {
        #[validate(length(min = 1))]
        value: String,
    }

    impl Initializer for InitRejected {
        fn try_initialize(&mut self) -> Result<(), ApiError> {
            Err(ApiError::conflict("init rejected"))
        }
    }

    /// An `Initializer` rejection passes through unchanged.
    #[tokio::test]
    async fn initializer_rejection_is_propagated() {
        let app = Router::new().route(
            "/",
            post(|Varser(value): Varser<InitRejected>| async move { value.value }),
        );
        let (status, body) = drive(app, json_request(r#"{"value":"x"}"#)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["message"], "init rejected");
    }

    #[derive(Debug, Deserialize, Validate)]
    struct Search {
        #[validate(length(min = 1, message = "required"))]
        q: String,
        limit: Option<u32>,
    }

    impl Initializer for Search {}

    /// Query: parse failure 400, rule failure 422.
    #[tokio::test]
    async fn query_extractor_splits_parse_from_validation() {
        // `limit` exists so that a non-numeric value is a *deserialization*
        // error, which is what the 400 case needs.
        let app = Router::new().route(
            "/search",
            get(|QueryVarser(search): QueryVarser<Search>| async move {
                format!("{}:{}", search.q, search.limit.unwrap_or(0))
            }),
        );

        let ok = Request::builder()
            .uri("/search?q=hello")
            .body(Body::empty())
            .unwrap();
        assert_eq!(drive(app.clone(), ok).await.0, StatusCode::OK);

        let invalid = Request::builder()
            .uri("/search?q=")
            .body(Body::empty())
            .unwrap();
        let (status, body) = drive(app.clone(), invalid).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["errors"]["q"][0]["message"], "required");

        // A query string that cannot be deserialized is a 400.
        let unparseable = Request::builder()
            .uri("/search?q=hello&limit=abc")
            .body(Body::empty())
            .unwrap();
        let (status, body) = drive(app, unparseable).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"], texts().bad_request.as_ref());
    }

    #[derive(Debug, Deserialize, Validate)]
    struct IdParam {
        #[validate(range(min = 1, message = "must be positive"))]
        id: u32,
    }

    impl Initializer for IdParam {}

    /// Path: parse failure 400 `BadRequest`, rule failure 422.
    #[tokio::test]
    async fn path_extractor_splits_parse_from_validation() {
        let app = Router::new().route(
            "/x/{id}",
            get(|PathVarser(param): PathVarser<IdParam>| async move { param.id.to_string() }),
        );

        let ok = Request::builder().uri("/x/42").body(Body::empty()).unwrap();
        assert_eq!(drive(app.clone(), ok).await.0, StatusCode::OK);

        let unparseable = Request::builder()
            .uri("/x/abc")
            .body(Body::empty())
            .unwrap();
        let (status, body) = drive(app.clone(), unparseable).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"], texts().bad_request.as_ref());

        let invalid = Request::builder().uri("/x/0").body(Body::empty()).unwrap();
        let (status, body) = drive(app, invalid).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["errors"]["id"][0]["message"], "must be positive");
    }

    #[derive(Debug, Deserialize, Validate)]
    struct FormBody {
        #[validate(length(min = 1, message = "required"))]
        name: String,
    }

    impl Initializer for FormBody {}

    /// Form: parse failure 400, rule failure 422.
    #[tokio::test]
    async fn form_extractor_splits_parse_from_validation() {
        let app = Router::new().route(
            "/form",
            post(|FormVarser(form): FormVarser<FormBody>| async move { form.name }),
        );

        let form_request = |body: &str| {
            Request::builder()
                .method("POST")
                .uri("/form")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        assert_eq!(
            drive(app.clone(), form_request("name=foo")).await.0,
            StatusCode::OK
        );

        let (status, body) = drive(app.clone(), form_request("name=")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["errors"]["name"][0]["message"], "required");

        // A urlencoded body without the declared field cannot be deserialized.
        let (status, body) = drive(app, form_request("other=x")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"], texts().bad_request.as_ref());
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

#[cfg(all(test, feature = "validation-garde"))]
mod garde_tests {
    use super::{GardeFormVarser, GardeVarser, Initializer};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::routing::post;
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
        #[garde(length(min = 3))]
        email: String,
    }

    impl Initializer for Credentials {}

    /// The garde extractors mirror the validator ones: 400 on parse, 422 with
    /// the structured payload on a rule failure.
    #[tokio::test]
    async fn garde_body_extractor_contract() {
        let app = Router::new().route(
            "/",
            post(|GardeVarser(creds): GardeVarser<Credentials>| async move { creds.email }),
        );

        let request = |body: &str| {
            Request::builder()
                .method("POST")
                .uri("/")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        assert_eq!(
            drive(app.clone(), request(r#"{"email":"ada@x.io"}"#))
                .await
                .0,
            StatusCode::OK
        );

        let (status, body) = drive(app.clone(), request(r#"{"email":42}"#)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], 400);

        let (status, body) = drive(app, request(r#"{"email":"a"}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["errors"]["email"][0]["code"], "invalid");
        assert!(
            body["errors"]["email"][0]["message"]
                .as_str()
                .is_some_and(|message| message.contains('3')),
            "garde's built-in message is kept: {body}"
        );
    }

    #[derive(Debug, Deserialize, garde::Validate)]
    struct FormBody {
        #[garde(length(min = 1))]
        name: String,
    }

    impl Initializer for FormBody {}

    /// The garde form extractor is wired as well.
    #[tokio::test]
    async fn garde_form_extractor_contract() {
        let app = Router::new().route(
            "/form",
            post(|GardeFormVarser(form): GardeFormVarser<FormBody>| async move { form.name }),
        );

        let request = |body: &str| {
            Request::builder()
                .method("POST")
                .uri("/form")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        assert_eq!(
            drive(app.clone(), request("name=foo")).await.0,
            StatusCode::OK
        );
        let (status, _) = drive(app, request("name=")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// Both backends exist side by side: a `validator` DTO still validates
    /// while the `garde` extractors are enabled (coherence would forbid one
    /// `Validate` bound covering both).
    #[cfg(feature = "validation-validator")]
    #[test]
    fn both_backends_coexist() {
        use validator::Validate as _;

        #[derive(Debug, validator::Validate)]
        struct Both {
            #[validate(length(min = 2))]
            name: String,
        }

        assert!(
            Both {
                name: "a".to_string()
            }
            .validate()
            .is_err()
        );
    }
}
