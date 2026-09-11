//! The single JSON envelope shared by success and error responses.
//!
//! Every handler returns [`ApiResponse`]; every failure is an
//! [`ApiError`](crate::error::ApiError). Both render the same body shape:
//!
//! ```json
//! { "code": 0, "message": "ok", "data": { "id": 1 } }
//! ```
//!
//! Rules, enforced by there being exactly one envelope type:
//!
//! - `code` is `0` on success and otherwise repeats the HTTP status code;
//! - `data` is always present, and `null` on error;
//! - `errors` appears only for a failed validation;
//! - `system` appears only for an internal error while debug mode is on (see
//!   [`install_debug_mode`](crate::install_debug_mode)).
//!
//! A success is always HTTP 200, matching the existing contract; a handler that
//! needs another status returns `(StatusCode, ApiResponse<T>)`.

use std::borrow::Cow;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::validation::ValidationErrors;

/// The `message` of every success body.
const OK_MESSAGE: &str = "ok";

/// The unified response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ApiResponse<T> {
    /// `0` on success, otherwise the HTTP status code of the failure.
    pub code: i32,
    /// A short human-readable message; `"ok"` on success.
    pub message: String,
    /// The validation violations; present only for a failed validation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<ValidationErrors>,
    /// The payload, `null` on error.
    pub data: Option<T>,
}

impl<T> ApiResponse<T> {
    /// A success body carrying `data`.
    ///
    /// ```
    /// use vivarium_web::response::ApiResponse;
    ///
    /// let body = ApiResponse::ok(serde_json::json!({ "id": 1 }));
    /// assert_eq!(body.code, 0);
    /// assert_eq!(body.message, "ok");
    /// ```
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            message: OK_MESSAGE.to_string(),
            errors: None,
            data: Some(data),
        }
    }

    /// An error body with no validation payload.
    ///
    /// `status` is copied into the body's `code` field only: the response
    /// still answers **HTTP 200**, because [`ApiResponse`] is the success
    /// path. Return an [`ApiError`](crate::ApiError) for a real error status,
    /// or pair this with `(StatusCode, ApiResponse<T>)`.
    pub fn error(status: i32, message: impl Into<String>) -> Self {
        Self {
            code: status,
            message: message.into(),
            errors: None,
            data: None,
        }
    }

    /// An error body carrying the structured `errors` of a failed validation.
    ///
    /// Like [`error`](ApiResponse::error), the HTTP status stays 200 — use
    /// [`ApiError::validation`](crate::ApiError::validation) to actually
    /// answer 422.
    pub fn error_with_errors(
        status: i32,
        message: impl Into<String>,
        errors: ValidationErrors,
    ) -> Self {
        Self {
            code: status,
            message: message.into(),
            errors: Some(errors),
            data: None,
        }
    }
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        let Self {
            code,
            message,
            errors,
            data,
        } = self;
        (
            StatusCode::OK,
            Json(envelope(
                code,
                Cow::Owned(message),
                errors.as_ref(),
                None,
                data,
            )),
        )
            .into_response()
    }
}

/// The one envelope shape, and the one place it is defined.
///
/// [`ApiResponse`] and [`ApiError`](crate::error::ApiError) both serialize
/// through this type, so field names, field order and the
/// `errors`-only-when-present rule cannot drift apart. `system` is a field of
/// this wire type only: it is never a field of [`ApiResponse`], and therefore
/// never part of the OpenAPI schema.
#[derive(Debug, Serialize)]
pub(crate) struct Envelope<'a, T> {
    /// `0` on success, otherwise the HTTP status code.
    code: i32,
    /// The response message.
    message: Cow<'a, str>,
    /// The validation violations, omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<&'a ValidationErrors>,
    /// Internal detail, omitted unless it is an internal error in debug mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    /// The payload; `null` for an error.
    data: Option<T>,
}

/// Builds the shared envelope.
///
/// `system` is `Some` only for an internal error in debug mode; `data` is
/// `Some` only on the success path.
pub(crate) fn envelope<'a, T: Serialize>(
    code: i32,
    message: Cow<'a, str>,
    errors: Option<&'a ValidationErrors>,
    system: Option<String>,
    data: Option<T>,
) -> impl Serialize + use<'a, T> {
    Envelope {
        code,
        message,
        errors,
        system,
        data,
    }
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

    /// A success is HTTP 200 and carries `code`, `message` and `data` — never
    /// `errors` or `system`.
    #[tokio::test]
    async fn success_is_the_documented_envelope() {
        let response = ApiResponse::ok(json!({ "id": 1 })).into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json(response).await,
            json!({ "code": 0, "message": "ok", "data": { "id": 1 } })
        );
    }

    /// The error envelope always carries `data: null` and omits `errors`.
    #[tokio::test]
    async fn error_keeps_data_and_omits_errors() {
        let response = ApiResponse::<()>::error(404, "not found").into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body,
            json!({ "code": 404, "message": "not found", "data": null })
        );
        assert!(body.get("errors").is_none());
        assert!(body.get("system").is_none());
    }

    /// A validation failure carries the structured `errors` payload.
    #[tokio::test]
    async fn validation_error_serializes_structured_errors() {
        let mut errors = ValidationErrors::new();
        errors.insert(
            "username",
            FieldViolation {
                code: "length".to_string(),
                message: Some("too short".to_string()),
                params: [("min".to_string(), json!(3))].into_iter().collect(),
            },
        );

        let response =
            ApiResponse::<()>::error_with_errors(422, "validation failed", errors).into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json(response).await,
            json!({
                "code": 422,
                "message": "validation failed",
                "errors": {
                    "username": [
                        { "code": "length", "message": "too short", "params": { "min": 3 } }
                    ]
                },
                "data": null,
            })
        );
    }

    /// A DTO that declares no message serializes the violation without one.
    #[test]
    fn violation_without_message_omits_the_key() {
        let mut errors = ValidationErrors::new();
        errors.insert(
            "email",
            FieldViolation {
                code: "email".to_string(),
                message: None,
                params: Default::default(),
            },
        );
        let body = serde_json::to_value(ApiResponse::<()>::error_with_errors(422, "nope", errors))
            .expect("serializes");
        assert_eq!(
            body["errors"]["email"][0],
            json!({ "code": "email", "params": {} })
        );
    }

    /// The envelope's field order is part of the contract.
    #[test]
    fn envelope_field_order_is_stable() {
        let rendered = serde_json::to_string(&ApiResponse::ok(json!(1))).expect("serializes");
        assert_eq!(rendered, r#"{"code":0,"message":"ok","data":1}"#);
    }
}
