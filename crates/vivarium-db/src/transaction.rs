//! Transaction helper: [`with_transaction`].
//!
//! The CRUD and query helpers take `impl Executor`, so the connection behind
//! the transaction works with all of them but one: [`Query::paginate`] needs
//! an executor that is `Copy` (it runs two queries), which the connection
//! inside a transaction is not. Inside a transaction, count first and page
//! with [`Query::paginate_with_total`]. This module only supplies the "begin,
//! run, commit or roll back" scaffold.
//!
//! [`Query::paginate`]: crate::Query::paginate
//! [`Query::paginate_with_total`]: crate::Query::paginate_with_total

use std::ops::AsyncFnOnce;

use sqlx::Pool;

use crate::{DriverOps, Error};

/// Runs `f` inside a database transaction.
///
/// Commits when `f` returns `Ok`, rolls back when it returns `Err` (a dropped
/// transaction is rolled back by sqlx as well).
///
/// The closure is an *async* closure (`async |tx| { … }`), which is what lets
/// the borrowed transaction be held across `await` points; a plain closure
/// returning an async block cannot express that borrow, and boxing the future
/// would push the cost onto every call site.
///
/// Inside the closure `tx` is `&mut Transaction`, so pass `&mut **tx` to an
/// executor-taking helper: the first `*` follows the reference, the second
/// derefs the transaction into the driver connection.
///
/// ```
/// # #[cfg(feature = "sqlite")] {
/// # use sqlx::SqlitePool;
/// # async fn example(pool: &SqlitePool) -> Result<(), sqlx::Error> {
/// use vivarium_db::with_transaction;
///
/// let inserted = with_transaction(pool, async |tx| {
///     sqlx::query("INSERT INTO users (name) VALUES ('a')")
///         .execute(&mut **tx)
///         .await?;
///     Ok(1_u64)
/// })
/// .await?;
/// assert_eq!(inserted, 1);
/// # Ok(())
/// # }
/// # }
/// ```
pub async fn with_transaction<'a, DB, F, T>(pool: &'a Pool<DB>, f: F) -> Result<T, Error>
where
    DB: DriverOps,
    F: AsyncFnOnce(&mut sqlx::Transaction<'a, DB>) -> Result<T, Error>,
{
    let mut tx = pool.begin().await?;
    let value = f(&mut tx).await?;
    tx.commit().await?;
    Ok(value)
}
