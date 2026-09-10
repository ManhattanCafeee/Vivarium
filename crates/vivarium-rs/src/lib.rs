//! # vivarium-rs
//!
//! The `vivarium-rs` facade: one dependency, feature-gated re-exports of the
//! whole family.
//!
//! Core types ([`Pagination`], [`Page`], [`Order`], [`Column`], [`Sorter`],
//! [`Entity`], [`Value`]) are always available. The `db`, `web`, and
//! `config` layers follow their cargo features:
//!
//! | Feature | Provides |
//! |---|---|
//! | `db` | [`Query`], [`Predicate`], [`Update`], [`with_transaction`], the CRUD helpers, and unique-violation detection, no driver |
//! | `db-sqlite` / `db-postgres` / `db-mysql` | driver-enabled `db` layer |
//! | `web` | [`ApiError`] / [`ApiResponse`], the `Varser` family, JWT + session auth, refresh tokens, RBAC permissions, password hashing, cache layer |
//! | `config` | [`Config`], [`ConfigOptions`], [`ConfigWatcher`] |
//! | `validation-garde` | the `Garde*` extractors (requires `web`) |
//! | `utoipa` / `utoipa-ui` | `ToSchema` derives plus the `openapi` helpers (requires `web` for the latter) |
//!
//! Default features: `web`, `config`, `db`, `db-sqlite`, `db-postgres`.
//!
//! Data-transfer objects are validated with [`validator`](https://docs.rs/validator) 0.20
//! (`Varser` requires `validator::Validate`); `db` additionally turns on
//! `vivarium-web/sqlx`, which provides `ApiError::conflict_from_db`.
//!
//! # Quick start (SQLite + axum)
//!
//! ```no_run
//! use axum::{Router, routing::post};
//! use serde::{Deserialize, Serialize};
//! use validator::Validate;
//! use vivarium_rs::sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
//! use vivarium_rs::{ApiError, ApiResponse, Initializer, Varser, create};
//!
//! #[derive(Clone, sqlx::FromRow, vivarium_rs::Entity)]
//! struct User {
//!     id: i64,
//!     name: String,
//! }
//!
//! #[derive(Deserialize, Validate)]
//! struct NewUser {
//!     #[validate(length(min = 1, max = 100, message = "name must be 1-100 characters"))]
//!     name: String,
//! }
//!
//! impl Initializer for NewUser {}
//!
//! #[derive(Serialize)]
//! struct UserJson {
//!     id: i64,
//!     name: String,
//! }
//!
//! async fn create_user(
//!     state: axum::extract::State<SqlitePool>,
//!     Varser(new_user): Varser<NewUser>,
//! ) -> Result<ApiResponse<UserJson>, ApiError> {
//!     let id = create(
//!         &state.0,
//!         User {
//!             id: 0,
//!             name: new_user.name.clone(),
//!         },
//!     )
//!     .await
//!     .map_err(ApiError::database)?;
//!     Ok(ApiResponse::ok(UserJson {
//!         id,
//!         name: new_user.name,
//!     }))
//! }
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = SqlitePoolOptions::new()
//!     .max_connections(1)
//!     .connect("sqlite::memory:")
//!     .await?;
//! let app: Router = Router::new()
//!     .route("/users", post(create_user))
//!     .with_state(pool);
//! # Ok(())
//! # }
//! ```
#![deny(missing_docs)]

pub use vivarium_core::{
    Column, EncodeError, Entity, NullType, Order, Page, Pagination, PrimaryKey, PrimaryKeyError,
    Sorter, Value,
};

#[cfg(feature = "db")]
pub use vivarium_db::{
    Error as DbError, Expr, Predicate, Query, RawFragment, RawFragmentError, Update, count, create,
    delete, exists, find_by_id, is_unique_violation, sqlx, update_by_id, with_transaction,
};

pub use vivarium_macros::Entity;

#[cfg(feature = "web")]
pub use vivarium_web::{
    AccessClaims, ApiError, ApiResponse, Argon2Params, CacheControl, Claims, CookieOptions,
    ErrorKind, FieldViolation, FormVarser, Initializer, JwtConfig, JwtVerifier, KeyRing,
    OptionalSessionCtx, PathVarser, PermissionSet, QueryVarser, RefreshTokenManager,
    RefreshTokenRecord, RefreshTokenStore, Result, SameSite, SessionAuth, SessionCtx, SessionId,
    SessionRecord, SessionStore, Texts, TokenPair, ValidationErrors, Varser, VerifyOutcome, authz,
    cache, debug_mode, error, get_authorization, hash, hash_token, hash_with, install_debug_mode,
    install_texts, jwt, needs_rehash, password, perms_match, response, secrets, serve,
    serve_with_shutdown, session, session_layer, should_extend, shutdown_signal, texts, token,
    validation, varser, verify, verify_and_upgrade, verify_login,
};

#[cfg(all(feature = "web", feature = "validation-garde"))]
pub use vivarium_web::{GardeFormVarser, GardePathVarser, GardeQueryVarser, GardeVarser};

#[cfg(all(feature = "web", feature = "utoipa"))]
pub use vivarium_web::openapi;

#[cfg(feature = "config")]
pub use vivarium_config::{Config, ConfigError, ConfigOptions, ConfigWatcher, HandlerId};
