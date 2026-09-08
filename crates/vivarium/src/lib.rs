//! # vivarium
//!
//! The `vivarium` facade: one dependency, feature-gated re-exports of the
//! whole family.
//!
//! Core types ([`Pagination`], [`Page`], [`Order`], [`Column`], [`Sorter`],
//! [`Entity`], [`Value`]) are always available. The `db`, `web`, and
//! `config` layers follow their cargo features:
//!
//! | Feature | Provides |
//! |---|---|
//! | `db` | [`Query`] and the CRUD helpers, no driver |
//! | `db-sqlite` / `db-postgres` / `db-mysql` | driver-enabled `db` layer |
//! | `web` | [`ApiError`], [`Varser`], JWT auth, cache layer |
//! | `config` | [`Config`] |
//!
//! Default features: `web`, `config`, `db`, `db-sqlite`, `db-postgres`.
//!
//! # Quick start (SQLite + axum)
//!
//! ```no_run
//! use axum::{Router, routing::post};
//! use serde::{Deserialize, Serialize};
//! use validator::Validate;
//! use vivarium::{ApiError, Order, Query, Sorter, Varser, create};
//! use vivarium::sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
//!
//! #[derive(Clone, sqlx::FromRow, vivarium::Entity)]
//! struct User {
//!     id: i64,
//!     name: String,
//! }
//!
//! #[derive(Debug, Clone, Copy)]
//! enum UserCol {
//!     Name,
//! }
//!
//! impl vivarium::Column for UserCol {
//!     fn name(&self) -> &'static str {
//!         match self {
//!             UserCol::Name => "name",
//!         }
//!     }
//! }
//!
//! #[derive(Deserialize, Validate)]
//! struct NewUser {
//!     #[validate(length(min = 1, max = 100))]
//!     name: String,
//! }
//!
//! impl vivarium::Initializer for NewUser {}
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
//! ) -> Result<(), ApiError> {
//!     create(&state.0, User { id: 0, name: new_user.name })
//!         .await
//!         .map_err(|e| ApiError::Internal { system: e.to_string() })?;
//!     Ok(())
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

pub use vivarium_core::{Column, Entity, Order, Page, Pagination, Sorter, Value};

#[cfg(feature = "db")]
pub use vivarium_db::{
    Error as DbError, MIGRATOR, Query, count, create, delete, exists, find_by_id, sqlx,
    update_by_id,
};

pub use vivarium_macros::Entity;

#[cfg(feature = "web")]
pub use vivarium_web::{
    ApiError, FormVarser, Initializer, PathVarser, QueryVarser, Varser, cache, get_authorization,
    jwt, serve,
};

#[cfg(feature = "config")]
pub use vivarium_config::{Config, ConfigError};
