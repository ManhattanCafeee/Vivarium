//! Chainable `SELECT` query builder: where/order/limit/offset clauses plus
//! `find`, `first`, `count`, and `paginate`.
//!
//! Clauses collect into a plain SQL string plus replayable bind values, so
//! building is side-effect free and repeatable — which is what makes `count`
//! plus page queries inside [`Query::paginate`] cheap and correct. All actual
//! query execution happens in per-driver impls (see [`crate::DriverOps`]);
//! nothing here touches a live connection.
//!
//! Bind values convert into the closed [`Value`] enum via `Into<Value>`:
//! strings (`&str` or `String`), integers, floats, bools, bytes, JSON, and
//! `Option`s of those are accepted. Values are bound by reference — nothing
//! is copied at execution time; only the where-clause steps are cloned per
//! build (`find` pays one clone, `paginate` two).
//!
//! Table and column names are quoted with the driver's syntax, so columns
//! named after SQL keywords (`order`, `desc`, …) are safe. [`Column`]
//! implementations carry logical names only.

use std::marker::PhantomData;

use sqlx::{Database, Executor, FromRow};
use vivarium_core::{Column, Entity, Order, Page, Pagination, Sorter, Value};

use crate::{DriverOps, Error, Step};

/// A chainable `SELECT` query over an [`Entity`] table.
///
/// Build it with [`new`], add clauses, then run [`find`], [`first`],
/// [`count`], or [`paginate`]. Bind values convert into the closed
/// [`Value`] enum (compile-time checked), and column names can only come
/// from a [`Column`] impl — no stringly-typed injection.
///
/// [`new`]: Query::new
/// [`find`]: Query::find
/// [`first`]: Query::first
/// [`count`]: Query::count
/// [`paginate`]: Query::paginate
pub struct Query<DB: Database, T> {
    steps: Vec<Step>,
    sorters: Vec<(String, Order)>,
    limit: Option<u64>,
    offset: Option<u64>,
    _db: PhantomData<fn() -> DB>,
    _marker: PhantomData<fn() -> T>,
}

impl<DB, T> Query<DB, T>
where
    DB: DriverOps,
    T: Entity,
{
    /// Starts an empty query over `T`'s table.
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            sorters: Vec::new(),
            limit: None,
            offset: None,
            _db: PhantomData,
            _marker: PhantomData,
        }
    }

    /// Adds an `AND col = value` clause (the first one becomes `WHERE`).
    ///
    /// The value converts into the closed [`Value`] enum — strings,
    /// integers, floats, bools, bytes, JSON, or `Option`s of those — so the
    /// bindable set is fixed at compile time. Integers are stored as
    /// [`Value::I64`]: `u64`/`usize` inputs above `i64::MAX` wrap silently.
    /// `None` binds as an `INT8`-typed NULL that PostgreSQL rejects against
    /// non-integer columns — for typed NULLs use raw `sqlx`.
    pub fn where_eq<C, V>(mut self, col: C, value: V) -> Self
    where
        C: Column,
        V: Into<Value>,
    {
        let prefix = if self.steps.is_empty() {
            " WHERE "
        } else {
            " AND "
        };
        self.steps.push(Step::Text(format!(
            "{prefix}{} = ",
            DB::quote_ident(col.name())
        )));
        self.steps.push(Step::Bind(value.into()));
        self
    }

    /// Adds an `ORDER BY` clause. Calling it multiple times sorts by multiple
    /// columns, in call order.
    pub fn order_by<C: Column>(mut self, sorter: Sorter<C>) -> Self {
        self.sorters
            .push((DB::quote_ident(sorter.col.name()), sorter.order));
        self
    }

    /// Limits the result rows.
    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    /// Skips the first `n` rows.
    pub fn offset(mut self, n: u64) -> Self {
        self.offset = Some(n);
        self
    }

    /// The SQL text this query will run, with driver placeholders rendered
    /// inline: `SELECT * FROM table` plus where, order, limit, and offset
    /// clauses.
    ///
    /// Useful for debugging and for unit tests that need no database.
    pub fn sql(&self) -> String {
        let prefix = format!("SELECT * FROM {}", DB::quote_ident(T::TABLE));
        DB::render_sql(&prefix, &self.steps, &self.tail())
    }

    /// The `ORDER BY` tail text, excluding the query's own limit/offset.
    fn order_tail(&self) -> String {
        let mut sql = String::new();
        if !self.sorters.is_empty() {
            sql.push_str(" ORDER BY ");
            for (i, (col, order)) in self.sorters.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                sql.push_str(col);
                sql.push(' ');
                sql.push_str(&order.to_string());
            }
        }
        sql
    }

    /// The full tail: `ORDER BY` plus the query's own limit/offset.
    fn tail(&self) -> String {
        let mut sql = self.order_tail();
        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if let Some(offset) = self.offset {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
        sql
    }

    /// The quoted `SELECT * FROM <table>` prefix.
    fn select_prefix() -> String {
        format!("SELECT * FROM {}", DB::quote_ident(T::TABLE))
    }

    /// The quoted `SELECT COUNT(*) FROM <table>` prefix.
    fn count_prefix() -> String {
        format!("SELECT COUNT(*) FROM {}", DB::quote_ident(T::TABLE))
    }

    /// Runs the query, returning all matching rows.
    pub async fn find<'e, E>(&self, db: E) -> Result<Vec<T>, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        DB::fetch_all(Self::select_prefix(), self.steps.clone(), self.tail(), db).await
    }

    /// Runs the query with `LIMIT 1`, returning the first row if any.
    pub async fn first<'e, E>(&self, db: E) -> Result<Option<T>, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        let suffix = format!("{} LIMIT 1", self.tail());
        DB::fetch_optional(Self::select_prefix(), self.steps.clone(), suffix, db).await
    }

    /// Counts rows matching this query's where clauses.
    pub async fn count<'e, E>(&self, db: E) -> Result<i64, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
    {
        DB::scalar_i64(Self::count_prefix(), self.steps.clone(), String::new(), db).await
    }

    /// Runs the query as one page: normalizes `pagination`, counts the total,
    /// and fetches exactly one page's rows (the query's own limit/offset are
    /// replaced by the pagination's).
    ///
    /// The executor must be `Copy` because two queries run against it; pass a
    /// reference (e.g. `&pool`).
    pub async fn paginate<'e, E>(&self, mut pagination: Pagination, db: E) -> Result<Page<T>, Error>
    where
        E: Executor<'e, Database = DB> + Copy + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        pagination.normalize();
        let total = self.count(db).await?.max(0) as u64;
        let (limit, offset) = pagination.limit_offset();
        let suffix = format!("{} LIMIT {limit} OFFSET {offset}", self.order_tail());
        let content = DB::fetch_all(Self::select_prefix(), self.steps.clone(), suffix, db).await?;
        Ok(Page {
            total,
            page: pagination.page,
            size: pagination.size,
            content,
        })
    }
}

impl<DB, T> Default for Query<DB, T>
where
    DB: DriverOps,
    T: Entity,
{
    /// Equivalent to [`Query::new`].
    fn default() -> Self {
        Self::new()
    }
}
