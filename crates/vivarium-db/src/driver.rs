//! Internal driver-specific glue.
//!
//! sqlx's `Arguments` and `Type` traits are concrete per driver and
//! `Type`/`Encode` impls do not exist for a generic `DB`, so scalar binds and
//! decodes (ids, counts, `Value`s) need one impl per driver. Additionally,
//! borrows into a `QueryBuilder<DB>` cannot cross `.await` in generic code
//! (rustc cannot see `DB::Arguments`'s destructor for drop-checking), so all
//! query execution lives in these per-driver impls.
//!
//! The execution futures are deliberately boxed (`Pin<Box<dyn Future +
//! Send>>`), not `impl Future`: type erasure keeps the executor parameter
//! `E` and its borrow lifetime out of the future types these methods return.
//! An `impl Future` return carries the `E: Executor` obligation in its type;
//! awaited inside an axum handler, rustc then cannot generalize `Send` over
//! the executor's lifetime and rejects the handler (compile errors citing
//! rustc #100013). Do not "optimize" these back to `impl Future` without
//! re-testing axum handlers. Everything here is sealed and hidden; the
//! public API only uses it as a bound.

use sqlx::types::Json;
use sqlx::{Database, Executor, QueryBuilder};
use std::future::Future;
use std::pin::Pin;
use vivarium_core::Value;

use crate::Error;

mod private {
    pub trait Sealed {}

    #[cfg(feature = "sqlite")]
    impl Sealed for sqlx::Sqlite {}
    #[cfg(feature = "postgres")]
    impl Sealed for sqlx::Postgres {}
    #[cfg(feature = "mysql")]
    impl Sealed for sqlx::MySql {}
}

/// One fragment of a query: literal SQL text, or a bind push.
///
/// Placeholders are never written into the text; [`QueryBuilder::push_bind`]
/// appends them with the driver's syntax (`?` vs `$1`), so text and binds
/// must be interleaved in execution order.
#[doc(hidden)]
#[derive(Clone)]
pub enum Step {
    /// Literal SQL text.
    Text(String),
    /// A bind value push.
    Bind(Value),
}

/// Driver-specific operations backing the generic CRUD and query API.
///
/// Implemented for SQLite, PostgreSQL, and MySQL (under their cargo
/// features). Sealed: downstream crates cannot implement it.
///
/// This trait appears as a bound on the public API so that scalar binds and
/// decodes type-check per driver; you never call it directly.
#[doc(hidden)]
pub trait DriverOps: Database + private::Sealed + Sized {
    /// The `RETURNING <id_col>` clause for drivers that support it; empty
    /// otherwise. The column name is quoted.
    fn returning_clause(id_col: &str) -> String;

    /// The affected-row count of a query result.
    fn rows_affected(result: &Self::QueryResult) -> u64;

    /// Quotes an identifier for SQL text: double quotes for SQLite and
    /// PostgreSQL, backticks for MySQL. Embedded quote characters are
    /// doubled.
    fn quote_ident(name: &str) -> String;

    /// Binds a [`Value`] by reference onto a query builder with
    /// driver-correct types. No values are copied; the builder encodes them
    /// into its argument buffer.
    fn bind_value<'a>(qb: &mut QueryBuilder<'a, Self>, value: &'a Value);

    /// Renders `prefix + clauses + suffix` with driver placeholders inlined,
    /// for debugging and SQL-text tests. Never touches a connection.
    fn render_sql(prefix: &str, clauses: &[Step], suffix: &str) -> String {
        let mut qb = QueryBuilder::<Self>::new(prefix.to_owned());
        push_steps(&mut qb, clauses);
        qb.push(suffix);
        qb.sql().to_owned()
    }

    /// Runs the built INSERT and returns the generated (or provided) id.
    fn generated_id<'c, E>(
        prefix: String,
        clauses: Vec<Step>,
        suffix: String,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c;

    /// Runs the query, returning all decoded rows.
    fn fetch_all<'c, E, T>(
        prefix: String,
        clauses: Vec<Step>,
        suffix: String,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<T>, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c,
        T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin;

    /// Runs the query, returning the first decoded row if any.
    fn fetch_optional<'c, E, T>(
        prefix: String,
        clauses: Vec<Step>,
        suffix: String,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<Option<T>, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c,
        T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin;

    /// Runs the query, returning the affected-row count.
    fn execute<'c, E>(
        prefix: String,
        clauses: Vec<Step>,
        suffix: String,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c;

    /// Runs the query, returning its single i64 column (COUNT etc.).
    fn scalar_i64<'c, E>(
        prefix: String,
        clauses: Vec<Step>,
        suffix: String,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c;

    /// Runs the query, returning its single bool column (EXISTS etc.).
    fn scalar_bool<'c, E>(
        prefix: String,
        clauses: Vec<Step>,
        suffix: String,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<bool, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c;
}

/// Per-driver extraction of the last-insert id from a query result.
#[cfg(feature = "sqlite")]
fn sqlite_rowid(result: &sqlx::sqlite::SqliteQueryResult) -> i64 {
    result.last_insert_rowid()
}

/// Per-driver extraction of the last-insert id from a query result.
#[cfg(feature = "mysql")]
fn mysql_rowid(result: &sqlx::mysql::MySqlQueryResult) -> i64 {
    result.last_insert_id() as i64
}

/// Placeholder: PostgreSQL fetches the id via `RETURNING`, so this is never
/// called (it only exists to satisfy the macro's dead branch).
#[cfg(feature = "postgres")]
fn pg_rowid(_result: &sqlx::postgres::PgQueryResult) -> i64 {
    0
}

macro_rules! impl_driver_ops {
    ($db:ty, $quote:literal, $returning:literal, $rowid:path) => {
        impl DriverOps for $db {
            fn returning_clause(id_col: &str) -> String {
                if $returning {
                    format!(" RETURNING {}", Self::quote_ident(id_col))
                } else {
                    String::new()
                }
            }

            fn rows_affected(result: &Self::QueryResult) -> u64 {
                result.rows_affected()
            }

            fn quote_ident(name: &str) -> String {
                let escaped = name.replace($quote, &$quote.repeat(2));
                format!("{}{}{}", $quote, escaped, $quote)
            }

            fn bind_value<'a>(qb: &mut QueryBuilder<'a, Self>, value: &'a Value) {
                match value {
                    Value::Null => {
                        // INT8-typed NULL: PostgreSQL rejects `col = $1`
                        // against non-integer columns at prepare time; use
                        // raw sqlx for typed NULLs.
                        qb.push_bind(Option::<i64>::None);
                    }
                    Value::I64(v) => {
                        qb.push_bind(*v);
                    }
                    Value::F64(v) => {
                        qb.push_bind(*v);
                    }
                    Value::Text(v) => {
                        qb.push_bind(v.as_str());
                    }
                    Value::Bytes(v) => {
                        qb.push_bind(v.as_slice());
                    }
                    Value::Bool(v) => {
                        qb.push_bind(*v);
                    }
                    Value::Json(v) => {
                        qb.push_bind(Json(v));
                    }
                }
            }

            fn generated_id<'c, E>(
                prefix: String,
                clauses: Vec<Step>,
                suffix: String,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(prefix);
                    push_steps(&mut qb, &clauses);
                    qb.push(&suffix);
                    if $returning {
                        let query = qb.build_query_scalar::<i64>();
                        query.fetch_one(db).await
                    } else {
                        let query = qb.build();
                        let result = query.execute(db).await?;
                        Ok($rowid(&result))
                    }
                })
            }

            fn fetch_all<'c, E, T>(
                prefix: String,
                clauses: Vec<Step>,
                suffix: String,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<T>, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
                T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(prefix);
                    push_steps(&mut qb, &clauses);
                    qb.push(&suffix);
                    let query = qb.build_query_as::<T>();
                    query.fetch_all(db).await
                })
            }

            fn fetch_optional<'c, E, T>(
                prefix: String,
                clauses: Vec<Step>,
                suffix: String,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<Option<T>, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
                T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(prefix);
                    push_steps(&mut qb, &clauses);
                    qb.push(&suffix);
                    let query = qb.build_query_as::<T>();
                    query.fetch_optional(db).await
                })
            }

            fn execute<'c, E>(
                prefix: String,
                clauses: Vec<Step>,
                suffix: String,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(prefix);
                    push_steps(&mut qb, &clauses);
                    qb.push(&suffix);
                    let query = qb.build();
                    let result = query.execute(db).await?;
                    Ok(Self::rows_affected(&result))
                })
            }

            fn scalar_i64<'c, E>(
                prefix: String,
                clauses: Vec<Step>,
                suffix: String,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(prefix);
                    push_steps(&mut qb, &clauses);
                    qb.push(&suffix);
                    let query = qb.build_query_scalar::<i64>();
                    query.fetch_one(db).await
                })
            }

            fn scalar_bool<'c, E>(
                prefix: String,
                clauses: Vec<Step>,
                suffix: String,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<bool, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(prefix);
                    push_steps(&mut qb, &clauses);
                    qb.push(&suffix);
                    let query = qb.build_query_scalar::<bool>();
                    query.fetch_one(db).await
                })
            }
        }
    };
}

/// Pushes text and bind steps onto a query builder in order. Binds are
/// pushed by reference; nothing is copied beyond the builder's own encoding.
fn push_steps<'a, DB: DriverOps>(qb: &mut QueryBuilder<'a, DB>, steps: &'a [Step]) {
    for step in steps {
        match step {
            Step::Text(text) => {
                qb.push(text);
            }
            Step::Bind(value) => {
                DB::bind_value(qb, value);
            }
        }
    }
}

#[cfg(feature = "sqlite")]
impl_driver_ops!(sqlx::Sqlite, "\"", false, sqlite_rowid);

#[cfg(feature = "postgres")]
impl_driver_ops!(sqlx::Postgres, "\"", true, pg_rowid);

#[cfg(feature = "mysql")]
impl_driver_ops!(sqlx::MySql, "`", false, mysql_rowid);
