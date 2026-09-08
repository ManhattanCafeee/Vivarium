//! Chainable `SELECT` query builder: where/order/limit/offset clauses plus
//! `find`, `first`, `count`, and `paginate`.
//!
//! Clauses collect into a plain SQL string plus replayable bind closures, so
//! building is side-effect free and repeatable — which is what makes `count`
//! plus page queries inside [`Query::paginate`] cheap and correct. All actual
//! query execution happens in per-driver impls (see [`crate::DriverOps`]);
//! nothing here touches a live connection.
//!
//! Bind values must be owned (`V: 'static`); pass `String` instead of
//! `&str`, and `Option<T>` instead of borrowed references.

use std::marker::PhantomData;

use sqlx::{Database, Encode, Executor, FromRow, Type};
use vivarium_core::{Column, Entity, Order, Page, Pagination, Sorter};

use crate::{DriverOps, Error, Step, TypedBinder};

/// A chainable `SELECT` query over an [`Entity`] table.
///
/// Build it with [`new`], add clauses, then run [`find`], [`first`],
/// [`count`], or [`paginate`]. Bind values are typed at compile time against
/// the driver, and column names can only come from a [`Column`] impl — no
/// stringly-typed injection.
///
/// [`new`]: Query::new
/// [`find`]: Query::find
/// [`first`]: Query::first
/// [`count`]: Query::count
/// [`paginate`]: Query::paginate
pub struct Query<DB: Database, T> {
    steps: Vec<Step<DB>>,
    sorters: Vec<(String, Order)>,
    limit: Option<u64>,
    offset: Option<u64>,
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
            _marker: PhantomData,
        }
    }

    /// Adds an `AND col = value` clause (the first one becomes `WHERE`).
    ///
    /// The value type must be encodable for the driver; mismatched types are
    /// rejected at compile time. Values must be owned and clonable because
    /// binds are replayed for count/page queries.
    pub fn where_eq<C, V>(mut self, col: C, value: V) -> Self
    where
        C: Column,
        V: for<'x> Encode<'x, DB> + Type<DB> + Clone + Send + Sync + 'static,
    {
        let prefix = if self.steps.is_empty() {
            " WHERE "
        } else {
            " AND "
        };
        self.steps
            .push(Step::Text(format!("{prefix}{} = ", col.name())));
        self.steps.push(Step::Bind(std::sync::Arc::new(TypedBinder {
            value,
            _db: PhantomData,
        })));
        self
    }

    /// Adds an `ORDER BY` clause. Calling it multiple times sorts by multiple
    /// columns, in call order.
    pub fn order_by<C: Column>(mut self, sorter: Sorter<C>) -> Self {
        self.sorters
            .push((sorter.col.name().to_owned(), sorter.order));
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
        let mut sql = format!("SELECT * FROM {}", T::TABLE);
        let mut index = 0;
        for step in &self.steps {
            match step {
                Step::Text(text) => sql.push_str(text),
                Step::Bind(_) => {
                    index += 1;
                    sql.push_str(&DB::placeholder(index));
                }
            }
        }
        self.push_tail(&mut sql);
        sql
    }

    /// Appends the order/limit/offset tail to a select SQL string.
    fn push_tail(&self, sql: &mut String) {
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
        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if let Some(offset) = self.offset {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
    }

    /// The step list of the full select query.
    fn select_steps(&self) -> Vec<Step<DB>> {
        let mut steps = vec![Step::Text(format!("SELECT * FROM {}", T::TABLE))];
        steps.extend(self.steps.iter().cloned());
        let mut tail = String::new();
        self.push_tail(&mut tail);
        if !tail.is_empty() {
            steps.push(Step::Text(tail));
        }
        steps
    }

    /// The step list of the matching `SELECT COUNT(*)` variant (order, limit,
    /// and offset are ignored).
    fn count_steps(&self) -> Vec<Step<DB>> {
        let mut steps = vec![Step::Text(format!("SELECT COUNT(*) FROM {}", T::TABLE))];
        steps.extend(self.steps.iter().cloned());
        steps
    }

    /// Runs the query, returning all matching rows.
    pub async fn find<'e, E>(&self, db: E) -> Result<Vec<T>, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        DB::fetch_all(self.select_steps(), db).await
    }

    /// Runs the query with `LIMIT 1`, returning the first row if any.
    pub async fn first<'e, E>(&self, db: E) -> Result<Option<T>, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
        T: for<'r> FromRow<'r, DB::Row> + Send + Unpin,
    {
        let mut steps = self.select_steps();
        steps.push(Step::Text(" LIMIT 1".to_owned()));
        DB::fetch_optional(steps, db).await
    }

    /// Counts rows matching this query's where clauses.
    pub async fn count<'e, E>(&self, db: E) -> Result<i64, Error>
    where
        E: Executor<'e, Database = DB> + 'e,
    {
        DB::scalar_i64(self.count_steps(), db).await
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
        let mut steps = self.select_steps();
        steps.push(Step::Text(format!(" LIMIT {limit} OFFSET {offset}")));
        let content = DB::fetch_all(steps, db).await?;
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
