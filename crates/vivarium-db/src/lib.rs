//! # vivarium-db
//!
//! Type-safe database ergonomics on top of [`sqlx`]: generic CRUD for
//! [`Entity`] types, a chainable query builder with compile-time-checked
//! bind values, and pagination built on [`Pagination`].
//!
//! ```no_run
//! use vivarium_db::{Column, Entity, Order, Pagination, Query, Sorter, create, find_by_id};
//! use sqlx::sqlite::SqlitePool;
//!
//! #[derive(Clone, sqlx::FromRow, vivarium_db::Entity)]
//! #[entity(crate = "vivarium_db")]
//! struct User {
//!     id: i64,
//!     name: String,
//! }
//!
//! #[derive(Clone, Copy)]
//! enum UserCol {
//!     Name,
//! }
//!
//! impl Column for UserCol {
//!     fn name(&self) -> &'static str {
//!         match self {
//!             UserCol::Name => "name",
//!         }
//!     }
//! }
//!
//! # async fn example(pool: SqlitePool) -> Result<(), sqlx::Error> {
//! let id = create(&pool, User { id: 0, name: "n".into() }).await?;
//! let user = find_by_id::<User, _>(&pool, id).await?;
//! let page = Query::<_, User>::new()
//!     .where_eq(UserCol::Name, "n".to_string())
//!     .order_by(Sorter::new(UserCol::Name, Order::Asc))
//!     .paginate(Pagination::new(1, 10), &pool)
//!     .await?;
//! assert_eq!(page.total, 1);
//! # Ok(())
//! # }
//! ```
//!
//! # Drivers
//!
//! Enable the `sqlite`, `postgres`, and/or `mysql` cargo features for the
//! corresponding driver. The whole crate compiles with none of them enabled;
//! everything except running queries works without a driver.
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs
#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod crud;
pub mod pool;
pub mod query;

mod driver;

#[doc(hidden)]
pub use driver::{Binder, BinderFor, DriverOps, Step, TypedBinder, ValueBinder};

pub use crud::{count, create, delete, exists, find_by_id, update_by_id};
pub use query::Query;
pub use vivarium_core::{Column, Entity, Order, Page, Pagination, Sorter, Value};
pub use vivarium_macros::Entity;

/// Re-export of the [`sqlx`] version this crate is built against, so users can
/// use the same version without re-declaring it.
pub use sqlx;

/// The error type for all fallible operations in this crate.
///
/// Currently a direct alias of [`sqlx::Error`]; driver and protocol errors
/// pass through unchanged.
pub type Error = sqlx::Error;

/// The embedded migrations of the example schema (`migrations/` directory).
///
/// Apply with [`sqlx::migrate::Migrator::run`] against a connection or pool.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
