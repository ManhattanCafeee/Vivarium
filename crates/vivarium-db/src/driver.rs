//! Internal driver-specific glue.
//!
//! sqlx's `Arguments` and `Type` traits are concrete per driver and
//! `Type`/`Encode` impls do not exist for a generic `DB`, so scalar binds and
//! decodes (ids, counts, `Value`s) need one impl per driver. Additionally,
//! borrows into a `QueryBuilder<DB>` cannot cross `.await` in generic code
//! (rustc cannot see `DB::Arguments`'s destructor for drop-checking), so all
//! query execution lives in these per-driver impls. Everything here is sealed
//! and hidden; the public API only uses it as a bound.

use sqlx::{Database, Encode, Executor, QueryBuilder, Type};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
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

/// A re-callable bind push: pushes one or more bind values onto a
/// [`QueryBuilder`] (the builder appends its own placeholders).
///
/// A trait object (not an HRTB `Fn` object) on purpose: HRTB closures make
/// futures holding them unprovably `Send` (rustc #100013), which would break
/// axum handlers.
#[doc(hidden)]
pub trait Binder: Send + Sync {
    /// The database this binder pushes values for.
    type DB: Database;

    /// Pushes this binder's values onto the query builder.
    fn bind(&self, qb: &mut QueryBuilder<'_, Self::DB>);
}

/// Binder alias with the database type fixed.
#[doc(hidden)]
pub type BinderFor<DB> = Arc<dyn Binder<DB = DB>>;

/// Binds one driver-typed value (used by [`Query::where_eq`]).
#[doc(hidden)]
pub struct TypedBinder<V, DB> {
    /// The value to bind.
    pub value: V,
    /// Type marker.
    pub _db: PhantomData<fn() -> DB>,
}

impl<V, DB> Binder for TypedBinder<V, DB>
where
    DB: Database,
    V: for<'x> Encode<'x, DB> + Type<DB> + Clone + Send + Sync + 'static,
{
    type DB = DB;

    fn bind(&self, qb: &mut QueryBuilder<'_, Self::DB>) {
        qb.push_bind(self.value.clone());
    }
}

/// Binds one [`Value`] with driver-correct types (used by the CRUD helpers).
#[doc(hidden)]
pub struct ValueBinder<DB> {
    /// The value to bind.
    pub value: Value,
    /// Type marker.
    pub _db: PhantomData<fn() -> DB>,
}

impl<DB: DriverOps> Binder for ValueBinder<DB> {
    type DB = DB;

    fn bind(&self, qb: &mut QueryBuilder<'_, Self::DB>) {
        DB::bind_value(qb, self.value.clone());
    }
}

/// One fragment of a query: literal SQL text, or a bind push.
///
/// Placeholders are never written into the text; [`QueryBuilder::push_bind`]
/// appends them with the driver's syntax (`?` vs `$1`), so text and binds
/// must be interleaved in execution order.
#[doc(hidden)]
pub enum Step<DB: Database> {
    /// Literal SQL text.
    Text(String),
    /// A bind value push.
    Bind(BinderFor<DB>),
}

impl<DB: Database> Clone for Step<DB> {
    fn clone(&self) -> Self {
        match self {
            Step::Text(text) => Step::Text(text.clone()),
            Step::Bind(bind) => Step::Bind(bind.clone()),
        }
    }
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
    /// The bind placeholder for the 1-based `index` (`?`, or `$1`, `$2`, ...).
    /// Used only for rendering SQL text for debugging/tests.
    fn placeholder(index: usize) -> String;

    /// The `RETURNING <id_col>` clause for drivers that support it; empty
    /// otherwise.
    fn returning_clause(id_col: &str) -> String;

    /// The affected-row count of a query result.
    fn rows_affected(result: &Self::QueryResult) -> u64;

    /// Binds a [`Value`] onto a query builder with driver-correct types.
    fn bind_value(qb: &mut QueryBuilder<'_, Self>, value: Value);

    /// Runs the built INSERT and returns the generated (or provided) id.
    fn generated_id<'c, E>(
        steps: Vec<Step<Self>>,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c;

    /// Runs the query, returning all decoded rows.
    fn fetch_all<'c, E, T>(
        steps: Vec<Step<Self>>,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<T>, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c,
        T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin;

    /// Runs the query, returning the first decoded row if any.
    fn fetch_optional<'c, E, T>(
        steps: Vec<Step<Self>>,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<Option<T>, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c,
        T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin;

    /// Runs the query, returning the affected-row count.
    fn execute<'c, E>(
        steps: Vec<Step<Self>>,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c;

    /// Runs the query, returning its single i64 column (COUNT etc.).
    fn scalar_i64<'c, E>(
        steps: Vec<Step<Self>>,
        db: E,
    ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
    where
        E: Executor<'c, Database = Self> + 'c;

    /// Runs the query, returning its single bool column (EXISTS etc.).
    fn scalar_bool<'c, E>(
        steps: Vec<Step<Self>>,
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
    ($db:ty, $dollar:literal, $returning:literal, $rowid:path) => {
        impl DriverOps for $db {
            fn placeholder(index: usize) -> String {
                if $dollar {
                    format!("${index}")
                } else {
                    "?".to_owned()
                }
            }

            fn returning_clause(id_col: &str) -> String {
                if $returning {
                    format!(" RETURNING {id_col}")
                } else {
                    String::new()
                }
            }

            fn rows_affected(result: &Self::QueryResult) -> u64 {
                result.rows_affected()
            }

            fn bind_value(qb: &mut QueryBuilder<'_, Self>, value: Value) {
                match value {
                    Value::Null => {
                        qb.push_bind(Option::<i64>::None);
                    }
                    Value::I64(v) => {
                        qb.push_bind(v);
                    }
                    Value::F64(v) => {
                        qb.push_bind(v);
                    }
                    Value::Text(v) => {
                        qb.push_bind(v);
                    }
                    Value::Bytes(v) => {
                        qb.push_bind(v);
                    }
                    Value::Bool(v) => {
                        qb.push_bind(v);
                    }
                    Value::Json(v) => {
                        qb.push_bind(v);
                    }
                }
            }

            fn generated_id<'c, E>(
                steps: Vec<Step<Self>>,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(String::new());
                    push_steps(&mut qb, &steps);
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
                steps: Vec<Step<Self>>,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<T>, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
                T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(String::new());
                    push_steps(&mut qb, &steps);
                    let query = qb.build_query_as::<T>();
                    query.fetch_all(db).await
                })
            }

            fn fetch_optional<'c, E, T>(
                steps: Vec<Step<Self>>,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<Option<T>, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
                T: for<'r> sqlx::FromRow<'r, Self::Row> + Send + Unpin,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(String::new());
                    push_steps(&mut qb, &steps);
                    let query = qb.build_query_as::<T>();
                    query.fetch_optional(db).await
                })
            }

            fn execute<'c, E>(
                steps: Vec<Step<Self>>,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(String::new());
                    push_steps(&mut qb, &steps);
                    let query = qb.build();
                    let result = query.execute(db).await?;
                    Ok(Self::rows_affected(&result))
                })
            }

            fn scalar_i64<'c, E>(
                steps: Vec<Step<Self>>,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<i64, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(String::new());
                    push_steps(&mut qb, &steps);
                    let query = qb.build_query_scalar::<i64>();
                    query.fetch_one(db).await
                })
            }

            fn scalar_bool<'c, E>(
                steps: Vec<Step<Self>>,
                db: E,
            ) -> Pin<Box<dyn Future<Output = Result<bool, Error>> + Send + 'c>>
            where
                E: Executor<'c, Database = Self> + 'c,
            {
                Box::pin(async move {
                    let mut qb = QueryBuilder::new(String::new());
                    push_steps(&mut qb, &steps);
                    let query = qb.build_query_scalar::<bool>();
                    query.fetch_one(db).await
                })
            }
        }
    };
}

/// Pushes text and bind steps onto a query builder in order.
fn push_steps<DB: Database>(qb: &mut QueryBuilder<'_, DB>, steps: &[Step<DB>]) {
    for step in steps {
        match step {
            Step::Text(text) => {
                qb.push(text);
            }
            Step::Bind(bind) => {
                bind.bind(qb);
            }
        }
    }
}

#[cfg(feature = "sqlite")]
impl_driver_ops!(sqlx::Sqlite, false, false, sqlite_rowid);

#[cfg(feature = "postgres")]
impl_driver_ops!(sqlx::Postgres, true, true, pg_rowid);

#[cfg(feature = "mysql")]
impl_driver_ops!(sqlx::MySql, false, false, mysql_rowid);
