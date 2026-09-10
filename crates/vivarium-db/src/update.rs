//! Partial-column `UPDATE` builder: [`Update`].
//!
//! Unlike [`update_by_id`](crate::update_by_id), which writes every non-id
//! column of an entity, `Update` writes exactly the columns you name — the
//! usual shape for "touch one field" operations such as
//! `Update::<User, UserCol>::new(id).set(UserCol::Name, "ada")`.
//!
//! Values are binds from the closed [`Value`] enum and column names can only
//! come from a [`Column`] implementation, so neither side of the statement can
//! be injected. The only non-bind SQL this builder can produce is
//! [`Expr::Now`], which is a fixed keyword.

use std::marker::PhantomData;

use sqlx::Executor;
use vivarium_core::{Column, Entity, PrimaryKey, Value};

use crate::{DriverOps, Error, Step};

/// A fixed SQL expression usable as an update value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Expr {
    /// The database's current timestamp (`CURRENT_TIMESTAMP`, portable across
    /// all supported drivers).
    Now,
}

impl Expr {
    /// The SQL text of this expression.
    fn sql(self) -> &'static str {
        match self {
            Self::Now => "CURRENT_TIMESTAMP",
        }
    }
}

/// The assigned value of one column.
#[derive(Debug, Clone, PartialEq)]
enum Assigned {
    /// A bound value.
    Value(Value),
    /// A fixed SQL expression.
    Expr(Expr),
}

/// A chainable `UPDATE` over a subset of `T`'s columns.
///
/// The column type `C` is not tied to `T` at the type level (the same is true
/// of [`Query`](crate::Query)); naming a column that does not exist on the
/// table is a runtime database error.
///
/// ```
/// use vivarium_core::Column;
/// use vivarium_db::Update;
///
/// #[derive(Clone, Copy)]
/// enum UserCol {
///     Name,
/// }
///
/// impl Column for UserCol {
///     fn name(&self) -> &'static str {
///         match self {
///             UserCol::Name => "name",
///         }
///     }
/// }
///
/// let update = Update::<User, UserCol>::new(1_i64).set(UserCol::Name, "ada");
/// # struct User { id: i64 }
/// # impl vivarium_core::Entity for User {
/// #     type Id = i64;
/// #     const TABLE: &'static str = "users";
/// #     const ID_COLUMN: &'static str = "id";
/// #     fn id(&self) -> i64 { self.id }
/// #     fn columns_and_values(&self)
/// #         -> Result<Vec<(&'static str, vivarium_core::Value)>, vivarium_core::EncodeError> {
/// #         Ok(Vec::new())
/// #     }
/// # }
/// # let _ = update;
/// ```
#[derive(Debug, Clone)]
pub struct Update<T: Entity, C: Column> {
    id: T::Id,
    sets: Vec<(C, Assigned)>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: Entity, C: Column> Update<T, C> {
    /// Starts an `UPDATE` for the row with the given primary key.
    pub fn new(id: T::Id) -> Self {
        Self {
            id,
            sets: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Sets `col` to `value`.
    pub fn set(mut self, col: C, value: impl Into<Value>) -> Self {
        self.sets.push((col, Assigned::Value(value.into())));
        self
    }

    /// Sets `col` to a fixed SQL expression (see [`Expr`]).
    pub fn set_expr(mut self, col: C, expr: Expr) -> Self {
        self.sets.push((col, Assigned::Expr(expr)));
        self
    }

    /// Runs the `UPDATE`, returning the affected-row count (`0` when no such
    /// row exists).
    ///
    /// Fails with [`sqlx::Error::Protocol`] when no column was set — an
    /// `UPDATE` with an empty `SET` list is invalid SQL, and silently doing
    /// nothing would hide the mistake.
    pub async fn execute<'e, DB, E>(self, db: E) -> Result<u64, Error>
    where
        DB: DriverOps,
        E: Executor<'e, Database = DB> + 'e,
    {
        if self.sets.is_empty() {
            return Err(Error::Protocol("Update has no columns to set".to_owned()));
        }
        let id = self.id.into_value().map_err(crate::key_error)?;

        let mut clauses: Vec<Step> = Vec::new();
        for (i, (col, assigned)) in self.sets.iter().enumerate() {
            if i > 0 {
                clauses.push(Step::Text(", ".to_owned()));
            }
            clauses.push(Step::Text(format!("{} = ", DB::quote_ident(col.name()))));
            match assigned {
                Assigned::Value(value) => clauses.push(Step::Bind(value.clone())),
                Assigned::Expr(expr) => clauses.push(Step::Text(expr.sql().to_owned())),
            }
        }
        clauses.push(Step::Text(format!(
            " WHERE {} = ",
            DB::quote_ident(T::ID_COLUMN)
        )));
        clauses.push(Step::Bind(id));

        let prefix = format!("UPDATE {} SET ", DB::quote_ident(T::TABLE));
        DB::execute(prefix, clauses, String::new(), db).await
    }
}
