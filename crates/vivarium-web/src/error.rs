//! The unified [`ApiError`] error type and its JSON response contract.

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// The shared error type returned by every `vivarium-web` extractor and handler.
///
/// It serializes to a stable JSON contract:
///
/// ```json
/// { "code": "NOT_FOUND", "message": "..." }
/// ```
///
/// plus a `system` field carrying internal details, but only while debug mode is
/// enabled (see [`set_debug_mode`]).
#[derive(thiserror::Error, Debug)]
pub enum ApiError {
    /// The requested resource does not exist.
    #[error("{0}")]
    NotFound(String),

    /// The request itself is malformed (bad body, bad token, ...).
    #[error("{0}")]
    BadRequest(String),

    /// The request payload failed deserialization or validation.
    #[error("{0}")]
    Validation(String),

    /// An unexpected server-side failure.
    #[error("internal")]
    Internal {
        /// Internal details, exposed to clients only while debug mode is on.
        system: String,
    },
}

impl ApiError {
    /// The HTTP status code to respond with for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The stable machine-readable error code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::NotFound(_) => "NOT_FOUND",
            ApiError::BadRequest(_) => "BAD_REQUEST",
            ApiError::Validation(_) => "VALIDATION",
            ApiError::Internal { .. } => "INTERNAL",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();
        let message = self.to_string();

        let mut body = serde_json::json!({
            "code": code,
            "message": message,
        });

        if debug_mode() {
            if let ApiError::Internal { system } = &self {
                body["system"] = serde_json::Value::String(system.clone());
            }
        }

        (status, Json(body)).into_response()
    }
}

/// Tracks whether debug mode is enabled.
///
/// Its initial value is parsed from the `VIVARIUM_DEBUG` environment variable
/// ("1", "true" or "on", case-insensitive) the first time it is read.
static DEBUG_MODE: AtomicBool = AtomicBool::new(false);

/// Ensures the initial value is parsed from the environment exactly once.
static DEBUG_MODE_INIT: Once = Once::new();

fn parse_debug_flag(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on"
    )
}

fn init_debug_mode() {
    DEBUG_MODE_INIT.call_once(|| {
        if let Ok(raw) = std::env::var("VIVARIUM_DEBUG") {
            DEBUG_MODE.store(parse_debug_flag(&raw), Ordering::Relaxed);
        }
    });
}

/// Enable or disable debug mode.
///
/// When enabled, [`ApiError::Internal`] responses include their `system`
/// details in the JSON body.
pub fn set_debug_mode(enabled: bool) {
    init_debug_mode();
    DEBUG_MODE.store(enabled, Ordering::Relaxed);
}

/// Report whether debug mode is currently enabled.
pub fn debug_mode() -> bool {
    init_debug_mode();
    DEBUG_MODE.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use serde_json::Value;

    async fn body_json(response: Response) -> Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("valid json body")
    }

    #[tokio::test]
    async fn statuses_codes_and_shape() {
        let cases = [
            (
                ApiError::NotFound("missing".into()),
                404,
                "NOT_FOUND",
                "missing",
            ),
            (
                ApiError::BadRequest("bad".into()),
                400,
                "BAD_REQUEST",
                "bad",
            ),
            (
                ApiError::Validation("bad email".into()),
                422,
                "VALIDATION",
                "bad email",
            ),
            (
                ApiError::Internal {
                    system: "boom".into(),
                },
                500,
                "INTERNAL",
                "internal",
            ),
        ];

        for (err, status, code, message) in cases {
            assert_eq!(err.status().as_u16(), status);
            assert_eq!(err.code(), code);
            let response = err.into_response();
            assert_eq!(response.status().as_u16(), status);
            let json = body_json(response).await;
            assert_eq!(json["code"], Value::String(code.to_string()));
            assert_eq!(json["message"], Value::String(message.to_string()));
        }
    }

    #[tokio::test]
    async fn debug_mode_toggles_system_field() {
        let err = || ApiError::Internal {
            system: "secret detail".to_string(),
        };

        set_debug_mode(false);
        let json = body_json(err().into_response()).await;
        assert!(
            json.get("system").is_none(),
            "system must be hidden: {json}"
        );

        set_debug_mode(true);
        let json = body_json(err().into_response()).await;
        assert_eq!(json["system"], Value::String("secret detail".to_string()));

        // A non-internal error never carries a system field, even in debug mode.
        set_debug_mode(true);
        let json = body_json(ApiError::NotFound("nope".into()).into_response()).await;
        assert!(json.get("system").is_none());

        set_debug_mode(false);
    }
}
