//! The unified [`ApiError`] and its JSON response contract.
//!
//! An [`ApiError`] renders the same envelope as
//! [`ApiResponse`](crate::response::ApiResponse):
//!
//! ```json
//! { "code": 404, "message": "not found", "data": null }
//! ```
//!
//! The [`ErrorKind`] is the machine-readable contract — it decides the HTTP
//! status code and the default message taken from
//! [`Texts`](crate::texts::Texts). Internal details never reach the client
//! unless debug mode is on, and then only in the `system` field.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::OnceLock;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::response::envelope;
use crate::texts::texts;
use crate::validation::ValidationErrors;

/// The category of an [`ApiError`].
///
/// The kind is stable and `match`able, unlike the stringly-typed `code` it
/// replaces; use [`ErrorKind::as_str`] for logs and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The request is malformed (bad path, query, form or body framing).
    BadRequest,
    /// The request body could not be deserialized.
    DataParse,
    /// The request is well-formed but semantically rejected.
    Validation,
    /// Authentication is required, or the credentials are invalid.
    Unauthorized,
    /// The authenticated principal lacks the required permission.
    Forbidden,
    /// The requested resource does not exist.
    NotFound,
    /// The request conflicts with the current state of the resource.
    Conflict,
    /// The client exceeded a rate limit.
    TooManyRequests,
    /// An unexpected server-side failure.
    Internal,
}

impl ErrorKind {
    /// The stable lower-case name of this kind, for logs and metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::DataParse => "data_parse",
            Self::Validation => "validation",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::TooManyRequests => "too_many_requests",
            Self::Internal => "internal",
        }
    }

    /// The HTTP status code this kind maps to.
    pub fn status(self) -> StatusCode {
        match self {
            Self::BadRequest | Self::DataParse => StatusCode::BAD_REQUEST,
            Self::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The shared error type of every `vivarium-web` extractor and handler.
///
/// Construct it with the per-kind constructor ([`ApiError::not_found`] and
/// friends) or with [`bail!`](crate::bail). The `message` is the one the client
/// sees; the failure that caused it lives in the `source`, which is logged and
/// — for [`ErrorKind::Internal`] in debug mode — echoed as `system`.
///
/// ```
/// use vivarium_web::error::ApiError;
///
/// let error = ApiError::not_found("no such user");
/// assert_eq!(error.status(), 404);
/// assert_eq!(error.message(), "no such user");
/// ```
#[derive(Debug)]
pub struct ApiError {
    kind: ErrorKind,
    message: Option<Cow<'static, str>>,
    errors: Option<ValidationErrors>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl ApiError {
    /// Builds an error of `kind` with an explicit `message`.
    ///
    /// Prefer the per-kind constructors, which document their status code;
    /// this exists for [`bail!`](crate::bail) and for code that dispatches on a
    /// kind it received.
    pub fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind,
            message: Some(message.into()),
            errors: None,
            source: None,
        }
    }

    /// A 400 for a malformed request (unparseable path, query or form data).
    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::BadRequest, message)
    }

    /// A 400 for a body that could not be deserialized.
    pub fn data_parse(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::DataParse, message)
    }

    /// A 422 carrying the failed validation rules.
    ///
    /// The message is the catalog's
    /// [`validation`](crate::texts::Texts::validation) text; the details travel
    /// in `errors`.
    pub fn validation(errors: ValidationErrors) -> Self {
        Self {
            kind: ErrorKind::Validation,
            message: None,
            errors: Some(errors),
            source: None,
        }
    }

    /// A 401: authentication is required or the credentials are invalid.
    pub fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::Unauthorized, message)
    }

    /// A 403: the principal is authenticated but not allowed.
    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::Forbidden, message)
    }

    /// A 404 for a missing resource.
    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    /// A 409 for a request that conflicts with the current state.
    pub fn conflict(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::Conflict, message)
    }

    /// A 429 for an exceeded rate limit.
    pub fn too_many_requests(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::TooManyRequests, message)
    }

    /// A 500 whose message is the catalog's
    /// [`internal`](crate::texts::Texts::internal) text.
    ///
    /// `source` is logged, and echoed as `system` while debug mode is on.
    pub fn internal(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            kind: ErrorKind::Internal,
            message: None,
            errors: None,
            source: Some(source.into()),
        }
    }

    /// A 500 caused by the database, whose message is the catalog's
    /// [`database`](crate::texts::Texts::database) text.
    ///
    /// Use this for storage failures the client must not see the detail of.
    pub fn database(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            kind: ErrorKind::Internal,
            message: Some(texts().database.clone()),
            errors: None,
            source: Some(source.into()),
        }
    }

    /// Attaches the underlying failure.
    ///
    /// The source is logged and, for an internal error in debug mode, exposed
    /// as `system`; it never becomes a 4xx `message`.
    #[must_use]
    pub fn with_source(self, source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            source: Some(source.into()),
            ..self
        }
    }

    /// The category of this error.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The HTTP status code of this error.
    pub fn status(&self) -> StatusCode {
        self.kind.status()
    }

    /// The client-facing message: the explicit one, or the catalog default of
    /// this kind.
    pub fn message(&self) -> &str {
        match &self.message {
            Some(message) => message,
            None => default_message(self.kind),
        }
    }

    /// The validation violations, for [`ErrorKind::Validation`] errors.
    pub fn errors(&self) -> Option<&ValidationErrors> {
        self.errors.as_ref()
    }

    /// Renders this error, exposing internal details when `debug` is set.
    ///
    /// [`IntoResponse`] uses [`debug_mode`]; this hook exists so tests need not
    /// touch the process-wide flag.
    #[doc(hidden)]
    pub fn into_response_with(self, debug: bool) -> Response {
        let kind = self.kind;
        let status = kind.status();
        let code = i32::from(status.as_u16());
        let system = if debug && kind == ErrorKind::Internal {
            self.source.as_ref().map(ToString::to_string)
        } else {
            None
        };
        log_failure(kind, self.source.as_deref());

        (
            status,
            Json(envelope(
                code,
                Cow::Borrowed(self.message()),
                self.errors.as_ref(),
                system,
                None::<()>,
            )),
        )
            .into_response()
    }
}

/// Logs the underlying failure next to the kind (never next to the message,
/// which is the client's).
fn log_failure(kind: ErrorKind, source: Option<&(dyn Error + Send + Sync)>) {
    match (kind, source) {
        (ErrorKind::Internal, Some(source)) => {
            tracing::error!(kind = kind.as_str(), error = %source, "request failed");
        }
        (ErrorKind::Internal, None) => {
            tracing::error!(kind = kind.as_str(), "request failed");
        }
        (_, Some(source)) => {
            tracing::warn!(kind = kind.as_str(), error = %source, "request rejected");
        }
        (_, None) => {}
    }
}

/// The catalog text for `kind`, used when no explicit message was supplied.
fn default_message(kind: ErrorKind) -> &'static str {
    let catalog = texts();
    match kind {
        ErrorKind::BadRequest => catalog.bad_request.as_ref(),
        ErrorKind::DataParse => catalog.data_parse.as_ref(),
        ErrorKind::Validation => catalog.validation.as_ref(),
        ErrorKind::Unauthorized => catalog.unauthorized.as_ref(),
        ErrorKind::Forbidden => catalog.forbidden.as_ref(),
        ErrorKind::NotFound => catalog.not_found.as_ref(),
        ErrorKind::Conflict => catalog.conflict.as_ref(),
        ErrorKind::TooManyRequests => catalog.too_many_requests.as_ref(),
        ErrorKind::Internal => catalog.internal.as_ref(),
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl Error for ApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.source {
            Some(source) => Some(&**source),
            None => None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.into_response_with(debug_mode())
    }
}

/// A message-only error `source`, for failures that arrive as text (a `serde`
/// detail, an axum rejection).
#[derive(Debug)]
struct Detail(String);

impl fmt::Display for Detail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Detail {}

/// Builds a 4xx error from an upstream parsing detail.
///
/// The detail becomes the `message` only while
/// [`Texts::echo_details`](crate::texts::Texts::echo_details) is enabled;
/// otherwise the catalog's text for `kind` is used and the detail stays in the
/// error source, where it is logged. A 4xx never carries a `system` field —
/// only [`ErrorKind::Internal`] does, and only in debug mode.
pub(crate) fn detail(kind: ErrorKind, detail: impl Into<String>) -> ApiError {
    let detail = detail.into();
    let message = if texts().echo_details {
        Cow::Owned(detail.clone())
    } else {
        Cow::Borrowed(default_message(kind))
    };
    ApiError {
        kind,
        message: Some(message),
        errors: None,
        source: Some(Box::new(Detail(detail))),
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        detail(ErrorKind::DataParse, error.to_string())
    }
}

#[cfg(feature = "sqlx")]
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if matches!(error, sqlx::Error::RowNotFound) {
            // Keep the driver error reachable: `log_failure` only logs errors
            // that carry a source, and a bare 404 tells an operator nothing.
            return Self::not_found(texts().not_found.clone()).with_source(error);
        }
        if error
            .as_database_error()
            .is_some_and(|database| database.is_unique_violation())
        {
            return Self::conflict(texts().conflict.clone()).with_source(error);
        }
        Self::database(error)
    }
}

#[cfg(feature = "sqlx")]
impl ApiError {
    /// Maps a database failure: a unique-constraint violation becomes a 409
    /// with `message`, anything else a 500.
    pub fn conflict_from_db(error: sqlx::Error, message: impl Into<Cow<'static, str>>) -> Self {
        if error
            .as_database_error()
            .is_some_and(|database| database.is_unique_violation())
        {
            Self::conflict(message).with_source(error)
        } else {
            Self::database(error)
        }
    }
}

/// Returns early with an [`ApiError`].
///
/// `bail!(kind, message)` builds that kind with an explicit message;
/// `bail!(message)` is shorthand for [`ApiError::internal`].
///
/// ```
/// use vivarium_web::{ApiError, ErrorKind, bail};
///
/// fn find(id: u64) -> Result<(), ApiError> {
///     if id == 0 {
///         bail!(ErrorKind::NotFound, "no such user");
///     }
///     if id == u64::MAX {
///         bail!("id overflowed");
///     }
///     Ok(())
/// }
///
/// assert_eq!(find(0).expect_err("missing").status(), 404);
/// assert_eq!(find(u64::MAX).expect_err("overflow").status(), 500);
/// ```
#[macro_export]
macro_rules! bail {
    ($kind:expr, $message:expr $(,)?) => {
        return ::core::result::Result::Err($crate::error::ApiError::new($kind, $message))
    };
    ($message:expr $(,)?) => {
        return ::core::result::Result::Err($crate::error::ApiError::internal($message))
    };
}

/// The library's [`Result`](std::result::Result), defaulting to [`ApiError`].
pub type Result<T, E = ApiError> = std::result::Result<T, E>;

/// Tracks whether internal details are exposed to clients.
static DEBUG_MODE: OnceLock<bool> = OnceLock::new();

/// Returned by [`install_debug_mode`] when the flag was already installed.
///
/// The first installation wins, so a repeated call is harmless and can be
/// ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("debug mode was already installed")]
pub struct DebugModeAlreadySet;

/// Installs the process-wide debug flag, once.
///
/// Call this at startup. While debug mode is on, [`ApiError`] responses with
/// kind [`ErrorKind::Internal`] carry their internal detail in `system`.
///
/// # Errors
///
/// Returns [`DebugModeAlreadySet`] if called more than once per process.
pub fn install_debug_mode(enabled: bool) -> Result<(), DebugModeAlreadySet> {
    DEBUG_MODE.set(enabled).map_err(|_| DebugModeAlreadySet)
}

/// Whether internal error details are exposed to clients.
///
/// Resolves once from `VIVARIUM_DEBUG` (`1`, `true` or `on`) when no value was
/// installed.
pub fn debug_mode() -> bool {
    *DEBUG_MODE.get_or_init(|| {
        std::env::var("VIVARIUM_DEBUG")
            .map(|raw| parse_debug_flag(&raw))
            .unwrap_or(false)
    })
}

/// The `VIVARIUM_DEBUG` values that mean "on".
fn parse_debug_flag(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::FieldViolation;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};

    async fn body_json(response: Response) -> Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("valid json body")
    }

    /// Every kind maps to its documented status and machine name.
    #[test]
    fn kinds_map_to_status_codes_and_names() {
        let cases = [
            (ErrorKind::BadRequest, 400, "bad_request"),
            (ErrorKind::DataParse, 400, "data_parse"),
            (ErrorKind::Validation, 422, "validation"),
            (ErrorKind::Unauthorized, 401, "unauthorized"),
            (ErrorKind::Forbidden, 403, "forbidden"),
            (ErrorKind::NotFound, 404, "not_found"),
            (ErrorKind::Conflict, 409, "conflict"),
            (ErrorKind::TooManyRequests, 429, "too_many_requests"),
            (ErrorKind::Internal, 500, "internal"),
        ];
        for (kind, status, name) in cases {
            assert_eq!(kind.status().as_u16(), status);
            assert_eq!(kind.as_str(), name);
            assert_eq!(kind.to_string(), name);
        }
    }

    /// Constructors set the documented kind, status and message.
    #[test]
    fn constructors_carry_kind_status_and_message() {
        let cases = [
            (ApiError::bad_request("bad"), 400, "bad"),
            (ApiError::data_parse("unparseable"), 400, "unparseable"),
            (ApiError::unauthorized("who?"), 401, "who?"),
            (ApiError::forbidden("nope"), 403, "nope"),
            (ApiError::not_found("missing"), 404, "missing"),
            (ApiError::conflict("taken"), 409, "taken"),
            (ApiError::too_many_requests("slow down"), 429, "slow down"),
        ];
        for (error, status, message) in cases {
            assert_eq!(error.status().as_u16(), status);
            assert_eq!(error.message(), message);
            assert!(!error.message().is_empty());
        }
    }

    /// Errors built without a message fall back to the catalog.
    #[test]
    fn catalog_supplies_missing_messages() {
        let validation = ApiError::validation(ValidationErrors::new());
        assert_eq!(validation.message(), texts().validation);
        assert_eq!(validation.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(validation.errors().is_some());

        let internal = ApiError::internal("boom");
        assert_eq!(internal.message(), texts().internal);
        assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(internal.to_string(), texts().internal);
        assert!(internal.errors().is_none());

        let database = ApiError::database("driver exploded");
        assert_eq!(database.message(), texts().database);
        assert_eq!(database.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The rendered error body is the response envelope, including `data: null`.
    #[tokio::test]
    async fn renders_the_response_envelope() {
        let response = ApiError::not_found("no such user").into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_json(response).await,
            json!({ "code": 404, "message": "no such user", "data": null })
        );
    }

    /// Field order is part of the byte-level contract: `code`, `message`,
    /// `errors?`, `system?`, `data`.
    #[tokio::test]
    async fn rendered_field_order_is_stable() {
        let rendered = async |response: Response| {
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("collect body")
                .to_bytes();
            String::from_utf8(bytes.to_vec()).expect("utf-8 body")
        };

        assert_eq!(
            rendered(ApiError::not_found("x").into_response()).await,
            r#"{"code":404,"message":"x","data":null}"#
        );

        let mut errors = ValidationErrors::new();
        errors.insert(
            "email",
            FieldViolation {
                code: "email".to_string(),
                message: None,
                params: Default::default(),
            },
        );
        assert_eq!(
            rendered(ApiError::validation(errors).into_response()).await,
            r#"{"code":422,"message":"validation failed","errors":{"email":[{"code":"email","params":{}}]},"data":null}"#
        );

        assert_eq!(
            rendered(ApiError::internal("boom").into_response_with(true)).await,
            r#"{"code":500,"message":"internal error","system":"boom","data":null}"#
        );
    }

    /// A validation failure carries the structured `errors`.
    #[tokio::test]
    async fn validation_renders_structured_errors() {
        let mut errors = ValidationErrors::new();
        errors.insert(
            "email",
            FieldViolation {
                code: "email".to_string(),
                message: Some("not an email".to_string()),
                params: Default::default(),
            },
        );
        let response = ApiError::validation(errors).into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body_json(response).await;
        assert_eq!(body["code"], 422);
        assert_eq!(body["message"], texts().validation.as_ref());
        assert_eq!(body["errors"]["email"][0]["code"], "email");
        assert_eq!(body["errors"]["email"][0]["message"], "not an email");
        assert_eq!(body["data"], Value::Null);
    }

    /// `system` is present only for internal errors, and only in debug mode.
    #[tokio::test]
    async fn system_field_is_internal_and_debug_only() {
        let internal = || ApiError::internal("secret detail");

        let body = body_json(internal().into_response_with(false)).await;
        assert_eq!(
            body,
            json!({ "code": 500, "message": "internal error", "data": null })
        );
        assert!(body.get("system").is_none());

        let body = body_json(internal().into_response_with(true)).await;
        assert_eq!(body["system"], "secret detail");
        assert_eq!(body["message"], "internal error");

        // A non-internal error never carries `system`, even in debug mode.
        let body = body_json(ApiError::not_found("nope").into_response_with(true)).await;
        assert!(body.get("system").is_none());
    }

    /// `with_source` never leaks a 4xx detail into `system`.
    #[tokio::test]
    async fn with_source_keeps_4xx_details_out_of_the_body() {
        let error = ApiError::bad_request("bad").with_source("the detail");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.message(), "bad");
        let body = body_json(error.into_response_with(true)).await;
        assert!(
            body.get("system").is_none(),
            "only internal errors expose details"
        );
    }

    /// The `From` table maps each source to its documented kind.
    #[test]
    fn from_table_maps_sources() {
        let io = ApiError::from(std::io::Error::other("disk on fire"));
        assert_eq!(io.kind(), ErrorKind::Internal);
        assert_eq!(io.message(), texts().internal);

        let json = ApiError::from(
            serde_json::from_str::<serde_json::Value>("{").expect_err("invalid json"),
        );
        assert_eq!(json.kind(), ErrorKind::DataParse);
        assert_eq!(json.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json.message(),
            texts().data_parse,
            "details are not echoed by default"
        );

        // `std::error::Error::source` keeps the detail reachable for logs.
        assert!(Error::source(&json).is_some());
    }

    /// `VIVARIUM_DEBUG` parsing accepts the documented spellings.
    #[test]
    fn debug_flag_parsing() {
        for raw in ["1", "true", "TRUE", " on "] {
            assert!(parse_debug_flag(raw), "{raw} should enable debug mode");
        }
        for raw in ["0", "false", "off", ""] {
            assert!(!parse_debug_flag(raw), "{raw} should not enable debug mode");
        }
    }

    /// `bail!` returns the error of the given kind.
    #[test]
    fn bail_macro() {
        fn failing(kind: ErrorKind) -> Result<()> {
            bail!(kind, "explicit");
        }
        assert_eq!(
            failing(ErrorKind::Conflict).expect_err("failed").kind(),
            ErrorKind::Conflict
        );

        fn internal() -> Result<()> {
            bail!("shorthand");
        }
        assert_eq!(internal().expect_err("failed").kind(), ErrorKind::Internal);
    }

    /// Database errors map to 404 / 409 / 500.
    #[cfg(feature = "sqlx")]
    #[test]
    fn sqlx_errors_map_to_kinds() {
        use sqlx::error::{DatabaseError, ErrorKind as SqlxKind};

        #[derive(Debug)]
        struct UniqueViolation;

        impl fmt::Display for UniqueViolation {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("duplicate key value violates unique constraint")
            }
        }

        impl Error for UniqueViolation {}

        impl DatabaseError for UniqueViolation {
            fn message(&self) -> &str {
                "duplicate key value violates unique constraint"
            }

            fn kind(&self) -> SqlxKind {
                SqlxKind::UniqueViolation
            }

            fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
                self
            }

            fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
                self
            }

            fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
                self
            }
        }

        let missing = ApiError::from(sqlx::Error::RowNotFound);
        assert_eq!(missing.kind(), ErrorKind::NotFound);

        let duplicate = ApiError::from(sqlx::Error::Database(Box::new(UniqueViolation)));
        assert_eq!(duplicate.kind(), ErrorKind::Conflict);

        let other = ApiError::from(sqlx::Error::Protocol("bad frame".to_string()));
        assert_eq!(other.kind(), ErrorKind::Internal);
        assert_eq!(other.message(), texts().database);

        let mapped = ApiError::conflict_from_db(
            sqlx::Error::Database(Box::new(UniqueViolation)),
            "already registered",
        );
        assert_eq!(mapped.kind(), ErrorKind::Conflict);
        assert_eq!(mapped.message(), "already registered");

        let fallback = ApiError::conflict_from_db(sqlx::Error::RowNotFound, "already registered");
        assert_eq!(fallback.kind(), ErrorKind::Internal);
    }
}
