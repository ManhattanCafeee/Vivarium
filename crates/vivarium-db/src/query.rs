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

use crate::predicate::Predicate;
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
    projection: Option<String>,
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
            projection: None,
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
    /// `None` binds as an untyped `INT8` NULL that PostgreSQL rejects against
    /// non-integer columns; pass [`Value::TypedNull`] with the matching
    /// [`NullType`](vivarium_core::NullType) for a typed NULL.
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

    /// Restricts the selected columns, replacing the default `SELECT *`.
    ///
    /// Every name comes from a [`Column`] impl. The selected columns must be
    /// enough for `T: FromRow` to decode — a missing column is a runtime
    /// decode error, not a compile-time one.
    ///
    /// An empty slice keeps `SELECT *` (an empty projection is not valid
    /// SQL), so a dynamically computed column list that ends up empty does
    /// not narrow the query.
    pub fn select<C: Column>(mut self, columns: &[C]) -> Self {
        let projection = columns
            .iter()
            .map(|col| DB::quote_ident(col.name()))
            .collect::<Vec<_>>()
            .join(", ");
        self.projection = Some(if projection.is_empty() {
            "*".to_owned()
        } else {
            projection
        });
        self
    }

    /// Adds a typed [`Predicate`]; repeated calls combine with `AND`.
    ///
    /// The predicate's columns are quoted with the driver's syntax and every
    /// value is bound, so the only way to reach raw SQL is
    /// [`raw_where`](Query::raw_where).
    pub fn filter<C: Column>(mut self, predicate: Predicate<C>) -> Self {
        let prefix = if self.steps.is_empty() {
            " WHERE "
        } else {
            " AND "
        };
        self.steps.push(Step::Text(prefix.to_owned()));
        predicate.render::<DB>(&mut self.steps);
        self
    }

    /// Adds a hand-written `WHERE` fragment — the explicit escape hatch.
    ///
    /// The fragment's SQL text uses `?` for each bind, in order; the
    /// placeholders are rewritten to the driver's own syntax when the query
    /// runs. `RawFragment::new` fails when the two counts disagree, so a
    /// fragment can never bind the wrong number of values.
    pub fn raw_where(mut self, fragment: RawFragment) -> Self {
        let prefix = if self.steps.is_empty() {
            " WHERE "
        } else {
            " AND "
        };
        self.steps.push(Step::Text(prefix.to_owned()));
        self.steps.push(Step::Text("(".to_owned()));
        for (index, part) in fragment.sql.split('?').enumerate() {
            if index > 0
                && let Some(value) = fragment.binds.get(index - 1)
            {
                self.steps.push(Step::Bind(value.clone()));
            }
            self.steps.push(Step::Text(part.to_owned()));
        }
        self.steps.push(Step::Text(")".to_owned()));
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
    /// inline: `SELECT <projection> FROM <table>` plus where, order, limit,
    /// and offset clauses (`<projection>` is `*` unless
    /// [`select`](Query::select) narrowed it).
    ///
    /// It reflects only the query's own [`limit`](Query::limit) and
    /// [`offset`](Query::offset). [`first`](Query::first) ignores both — its
    /// SQL is the `ORDER BY` tail plus `LIMIT 1`, i.e. `SELECT … FROM …
    /// ORDER BY … LIMIT 1`, never the clauses shown here — and
    /// [`paginate`](Query::paginate) replaces both with the pagination's
    /// values.
    ///
    /// Useful for debugging and for unit tests that need no database.
    pub fn sql(&self) -> String {
        DB::render_sql(&self.select_prefix(), &self.steps, &self.tail())
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

    /// The quoted `SELECT … FROM <table>` prefix, honouring [`select`].
    ///
    /// [`select`]: Query::select
    fn select_prefix(&self) -> String {
        let columns = self.projection.as_deref().unwrap_or("*");
        format!("SELECT {columns} FROM {}", DB::quote_ident(T::TABLE))
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
        DB::fetch_all(self.select_prefix(), self.steps.clone(), self.tail(), db).await
    }

    /// Runs the query with `LIMIT 1`, returning the first row if any.
    ///
    /// The query's own [`limit`](Query::limit)/[`offset`](Query::offset) are
    /// ignored: appending them would produce invalid SQL such as
    /// `LIMIT 10 OFFSET 5 LIMIT 1`. Only `ORDER BY` and the added `LIMIT 1`
    /// are emitted, so `first()` returns the first row of the ordering.
    pub async fn first<'e, E>(&self, db: E) -> Result<Option<T>, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        let suffix = format!("{} LIMIT 1", self.order_tail());
        DB::fetch_optional(self.select_prefix(), self.steps.clone(), suffix, db).await
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
    ///
    /// A transaction offers no `Copy` executor, so inside one take the total
    /// first with [`count`](Query::count) (or reuse a known one) and page with
    /// [`paginate_with_total`](Query::paginate_with_total) instead. Hand the
    /// transaction connection over as `&mut **tx`, the convention of
    /// [`with_transaction`](crate::with_transaction).
    pub async fn paginate<'e, E>(&self, mut pagination: Pagination, db: E) -> Result<Page<T>, Error>
    where
        E: Executor<'e, Database = DB> + Copy + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        pagination.normalize();
        let total = self.count(db).await?.max(0) as u64;
        let (limit, offset) = pagination.limit_offset();
        let suffix = format!("{} LIMIT {limit} OFFSET {offset}", self.order_tail());
        let items = DB::fetch_all(self.select_prefix(), self.steps.clone(), suffix, db).await?;
        Ok(Page {
            items,
            total,
            page: pagination.page,
            per_page: pagination.per_page,
        })
    }

    /// Like [`paginate`](Query::paginate) but with a total the caller already
    /// knows, so only the page query runs.
    ///
    /// Use it when the count and the page come from different places (or when
    /// the total is cached); the two-query [`paginate`](Query::paginate) is
    /// the right default otherwise.
    pub async fn paginate_with_total<'e, E>(
        &self,
        mut pagination: Pagination,
        total: u64,
        db: E,
    ) -> Result<Page<T>, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        pagination.normalize();
        let (limit, offset) = pagination.limit_offset();
        let suffix = format!("{} LIMIT {limit} OFFSET {offset}", self.order_tail());
        let items = DB::fetch_all(self.select_prefix(), self.steps.clone(), suffix, db).await?;
        Ok(Page {
            items,
            total,
            page: pagination.page,
            per_page: pagination.per_page,
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

/// A hand-written SQL fragment for [`Query::raw_where`].
///
/// This is the explicit escape hatch of the query builder: the SQL text is
/// yours, so nothing but the `?`/bind count is checked for you. Prefer
/// [`Predicate`] whenever it can express the condition.
///
/// ```
/// use vivarium_db::RawFragment;
///
/// let fragment = RawFragment::new("age > ? AND lower(name) = ?", vec![18_i64.into(), "ada".into()])
///     .expect("one bind per placeholder");
/// assert_eq!(fragment.sql(), "age > ? AND lower(name) = ?");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RawFragment {
    sql: String,
    binds: Vec<Value>,
}

impl RawFragment {
    /// Builds a fragment, checking that the number of `?` placeholders
    /// matches the number of binds.
    ///
    /// `?` is reserved: every occurrence is treated as a placeholder, so a
    /// literal question mark (a `'a?b'` string literal, PostgreSQL's JSONB `?`
    /// operator) cannot appear in a fragment — write those queries with raw
    /// sqlx (or an equivalent function call, such as `jsonb_exists`) instead.
    ///
    /// # Errors
    ///
    /// Returns [`RawFragmentError`] when the counts differ.
    pub fn new(sql: impl Into<String>, binds: Vec<Value>) -> Result<Self, RawFragmentError> {
        let sql = sql.into();
        let placeholders = sql.matches('?').count();
        if placeholders != binds.len() {
            return Err(RawFragmentError {
                placeholders,
                binds: binds.len(),
            });
        }
        Ok(Self { sql, binds })
    }

    /// The SQL text, with `?` placeholders.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// The bind values, in placeholder order.
    pub fn binds(&self) -> &[Value] {
        &self.binds
    }
}

/// The error returned by [`RawFragment::new`] when the placeholder and bind
/// counts disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawFragmentError {
    placeholders: usize,
    binds: usize,
}

impl RawFragmentError {
    /// The number of `?` placeholders in the SQL text.
    pub fn placeholders(&self) -> usize {
        self.placeholders
    }

    /// The number of binds supplied.
    pub fn binds(&self) -> usize {
        self.binds
    }
}

impl std::fmt::Display for RawFragmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "raw fragment has {} `?` placeholders but {} binds",
            self.placeholders, self.binds
        )
    }
}

impl std::error::Error for RawFragmentError {}

impl From<RawFragmentError> for Error {
    /// Lets `RawFragment::new(..)?` compose with the crate's error type, the
    /// same way [`EncodeError`](vivarium_core::EncodeError) does.
    fn from(error: RawFragmentError) -> Self {
        Error::Encode(Box::new(error))
    }
}
