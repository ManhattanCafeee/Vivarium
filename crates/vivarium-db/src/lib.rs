//! # vivarium-db
//!
//! Type-safe database ergonomics on top of [`sqlx`]: generic CRUD for
//! [`Entity`] types, a chainable query builder with compile-time-checked
//! bind values, a typed [`Predicate`] filter AST (with a documented raw
//! escape hatch), partial-column [`Update`]s, transactions, and pagination
//! built on [`Pagination`].
//!
//! # Migrations
//!
//! This crate ships **no** embedded migrator: a library-owned
//! `_sqlx_migrations` table would collide with the host application's, and the
//! example schema is SQLite-flavoured. Use [`sqlx::migrate!`] from the
//! application (see `examples/migrations/` for a sample layout).
//!
//! ```no_run
//! # #[cfg(feature = "sqlite")] {
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
pub mod predicate;
pub mod query;
pub mod transaction;
pub mod update;

mod driver;

#[doc(hidden)]
pub use driver::{DriverOps, Step};

pub use crud::{count, create, delete, exists, find_by_id, update_by_id};
pub use predicate::Predicate;
pub use query::{Query, RawFragment, RawFragmentError};
pub use transaction::with_transaction;
pub use update::{Expr, Update};
pub use vivarium_core::{
    Column, EncodeError, Entity, NullType, Order, Page, Pagination, PrimaryKey, PrimaryKeyError,
    Sorter, Value,
};
pub use vivarium_macros::Entity;

/// Re-export of the [`sqlx`] version this crate is built against, so users can
/// use the same version without re-declaring it.
pub use sqlx;

/// The error type for all fallible operations in this crate.
///
/// Currently a direct alias of [`sqlx::Error`]; driver and protocol errors
/// pass through unchanged.
pub type Error = sqlx::Error;

/// Converts an entity encoding failure into a driver error.
///
/// `#[derive(Entity)]` reports a field that cannot be encoded (an
/// `#[entity(json)]` field whose `Serialize` fails, for instance) as
/// [`EncodeError`](vivarium_core::EncodeError); the CRUD helpers surface it as
/// [`sqlx::Error::Encode`] so callers can keep matching on one error type.
pub(crate) fn encode_error(error: vivarium_core::EncodeError) -> Error {
    Error::Encode(Box::new(error))
}

/// Converts a primary-key conversion failure into a driver error.
pub(crate) fn key_error(error: vivarium_core::PrimaryKeyError) -> Error {
    Error::Encode(Box::new(error))
}

/// Returns true if `err` is a unique or primary-key constraint violation.
///
/// Delegates to sqlx's driver-agnostic
/// [`sqlx::error::DatabaseError::is_unique_violation`]; with a driver feature
/// enabled, MySQL (1062), PostgreSQL (23505), and SQLite (extended
/// `SQLITE_CONSTRAINT_UNIQUE`/`SQLITE_CONSTRAINT_PRIMARYKEY`) all report it.
pub fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}

#[cfg(all(test, feature = "sqlite"))]
mod violation_tests {
    use super::*;

    #[tokio::test]
    async fn unique_violation_detected() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT UNIQUE)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (id, name) VALUES (1, 'a')")
            .execute(&pool)
            .await
            .unwrap();

        let dup_name = sqlx::query("INSERT INTO t (id, name) VALUES (2, 'a')")
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(is_unique_violation(&dup_name));

        let dup_pk = sqlx::query("INSERT INTO t (id, name) VALUES (1, 'b')")
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(is_unique_violation(&dup_pk));

        assert!(!is_unique_violation(&sqlx::Error::RowNotFound));
    }
}
