//! # vivarium-web
//!
//! HTTP ergonomics on top of [`axum`]: one response envelope
//! ([`ApiResponse`] / [`ApiError`]), extractors that deserialize +
//! initialize + validate in one step, JWT and cookie-session authentication,
//! single-use refresh-token rotation, RBAC permission wildcards, password
//! hashing, a `Cache-Control` layer, and OpenAPI helpers.
//!
//! ## The response contract
//!
//! Every handler returns [`ApiResponse<T>`]; every failure is an [`ApiError`].
//! Both render the same body, defined by exactly one type:
//!
//! ```json
//! { "code": 0, "message": "ok", "data": { "id": 1 } }
//! { "code": 404, "message": "not found", "data": null }
//! { "code": 422, "message": "validation failed",
//!   "errors": { "username": [{ "code": "length", "message": "too short",
//!                              "params": { "min": 3, "max": 20 } }] },
//!   "data": null }
//! ```
//!
//! `code` is `0` on success and otherwise the HTTP status code; `data` is
//! always present; `errors` appears only for a failed validation; `system`
//! appears only for an internal error while debug mode is on. The envelope
//! `message` comes from [`Texts`] (English by default, installable once per
//! process) unless the call site supplied its own — a handler's
//! `ApiError::not_found("no such user")` reaches the client verbatim — while a
//! violation's own `message` is whatever the DTO declared in its
//! `#[validate(message = "…")]` attribute. The submitted field value is
//! never echoed back in `params`.
//!
//! ## Extractors
//!
//! | Extractor | Source | Failure |
//! |---|---|---|
//! | `Varser<T>` | JSON body | 400 `DataParse`, or 422 `Validation` |
//! | `QueryVarser<T>` | query string | 400 `BadRequest`, or 422 |
//! | `PathVarser<T>` | path parameters | 400 `BadRequest`, or 422 |
//! | `FormVarser<T>` | urlencoded body | 400 `BadRequest`, or 422 |
//!
//! They are gated by feature: `validation-validator` (default) binds
//! `validator::Validate`, `validation-garde` adds the `Garde*` equivalents
//! bound to `garde::Validate`.
//!
//! ## Features
//!
//! | Feature | Default | Adds |
//! |---|---|---|
//! | `validation-validator` | yes | the canonical `*Varser` extractors |
//! | `validation-garde` | no | the `Garde*` extractors |
//! | `utoipa` | no | `ToSchema` derives, `openapi` schemes and `info` |
//! | `utoipa-ui` | no | `openapi::mount` (Scalar + Swagger UI) |
//! | `sqlx` | no | `From<sqlx::Error> for ApiError`, `ApiError::conflict_from_db` |
//! | `telemetry` | no | `serve::telemetry::init` (stdout + JSON log files) |
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs

#![deny(missing_docs)]

pub mod authz;
pub mod cache;
pub mod error;
pub mod jwt;
pub mod password;
pub mod response;
pub mod secrets;
pub mod serve;
pub mod session;
pub mod texts;
pub mod token;
pub mod validation;
pub mod varser;

#[cfg(feature = "utoipa")]
pub mod openapi;

pub use authz::{PermissionSet, perms_match};
pub use cache::CacheControl;
pub use error::{ApiError, ErrorKind, Result, debug_mode, install_debug_mode};
pub use jwt::{JwtConfig, JwtVerifier, KeyRing};
pub use password::{
    Argon2Params, VerifyOutcome, hash, hash_with, needs_rehash, verify, verify_and_upgrade,
    verify_login,
};
pub use response::ApiResponse;
pub use secrets::hash_token;
pub use serve::{serve_with_shutdown, shutdown_signal};
pub use session::{
    CookieOptions, OptionalSessionCtx, SameSite, SessionAuth, SessionCtx, SessionId, SessionRecord,
    SessionStore, session_layer, should_extend,
};
pub use texts::{Texts, install_texts, texts};
pub use token::{
    AccessClaims, Claims, RefreshTokenManager, RefreshTokenRecord, RefreshTokenStore, TokenPair,
};
pub use validation::{FieldViolation, ValidationErrors};
pub use varser::{Initializer, get_authorization};

#[cfg(feature = "validation-validator")]
pub use varser::{FormVarser, PathVarser, QueryVarser, Varser};

#[cfg(feature = "validation-garde")]
pub use varser::{GardeFormVarser, GardePathVarser, GardeQueryVarser, GardeVarser};
