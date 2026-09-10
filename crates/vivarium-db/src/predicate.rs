//! A typed filter AST for [`Query::filter`].
//!
//! Column names can only come from a [`Column`] implementation and every
//! value is bound, never spliced into SQL text, so a predicate cannot be used
//! to inject SQL. The AST is a closed enum: there is no "raw SQL" escape
//! hatch here — that is [`Query::raw_where`], and it is explicit.
//!
//! ```
//! use vivarium_core::Column;
//! use vivarium_db::Predicate;
//!
//! #[derive(Clone, Copy)]
//! enum UserCol {
//!     Name,
//!     Age,
//! }
//!
//! impl Column for UserCol {
//!     fn name(&self) -> &'static str {
//!         match self {
//!             UserCol::Name => "name",
//!             UserCol::Age => "age",
//!         }
//!     }
//! }
//!
//! let predicate = Predicate::and([
//!     Predicate::starts_with(UserCol::Name, "ada"),
//!     Predicate::ge(UserCol::Age, 18_i64),
//! ]);
//! ```
//!
//! [`Query::filter`]: crate::Query::filter
//! [`Query::raw_where`]: crate::Query::raw_where

use vivarium_core::{Column, Value};

use crate::{DriverOps, Step};

/// The escape character used by the value-building helpers
/// ([`starts_with`][Predicate::starts_with] and friends). `!` is used instead
/// of a backslash because backslashes in string literals differ between MySQL
/// and PostgreSQL.
const LIKE_ESCAPE: char = '!';

/// A typed filter predicate over columns of type `C`.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate<C: Column> {
    /// `col = value`.
    Eq(C, Value),
    /// `col <> value`.
    Ne(C, Value),
    /// `col < value`.
    Lt(C, Value),
    /// `col <= value`.
    Le(C, Value),
    /// `col > value`.
    Gt(C, Value),
    /// `col >= value`.
    Ge(C, Value),
    /// `col LIKE pattern`; wildcards in `pattern` stay active.
    Like(C, String),
    /// `col NOT LIKE pattern`; wildcards in `pattern` stay active.
    NotLike(C, String),
    /// `col LIKE 'value%'`, with `%`/`_` inside `value` treated literally.
    StartsWith(C, String),
    /// `col LIKE '%value'`, with `%`/`_` inside `value` treated literally.
    EndsWith(C, String),
    /// `col LIKE '%value%'`, with `%`/`_` inside `value` treated literally.
    Contains(C, String),
    /// `col IN (…)`; an empty list matches nothing (`1 = 0`).
    In(C, Vec<Value>),
    /// `col NOT IN (…)`; an empty list matches everything (`1 = 1`).
    NotIn(C, Vec<Value>),
    /// `col IS NULL`.
    IsNull(C),
    /// `col IS NOT NULL`.
    IsNotNull(C),
    /// All children, combined with `AND`; an empty list matches everything.
    And(Vec<Predicate<C>>),
    /// Any child, combined with `OR`; an empty list matches nothing.
    Or(Vec<Predicate<C>>),
    /// Negates the inner predicate.
    Not(Box<Predicate<C>>),
}

impl<C: Column> Predicate<C> {
    /// `col = value`.
    pub fn eq(col: C, value: impl Into<Value>) -> Self {
        Self::Eq(col, value.into())
    }

    /// `col <> value`.
    pub fn ne(col: C, value: impl Into<Value>) -> Self {
        Self::Ne(col, value.into())
    }

    /// `col < value`.
    pub fn lt(col: C, value: impl Into<Value>) -> Self {
        Self::Lt(col, value.into())
    }

    /// `col <= value`.
    pub fn le(col: C, value: impl Into<Value>) -> Self {
        Self::Le(col, value.into())
    }

    /// `col > value`.
    pub fn gt(col: C, value: impl Into<Value>) -> Self {
        Self::Gt(col, value.into())
    }

    /// `col >= value`.
    pub fn ge(col: C, value: impl Into<Value>) -> Self {
        Self::Ge(col, value.into())
    }

    /// `col LIKE pattern`, passing `pattern` through unchanged so its `%` and
    /// `_` wildcards stay active.
    pub fn like(col: C, pattern: impl Into<String>) -> Self {
        Self::Like(col, pattern.into())
    }

    /// `col NOT LIKE pattern`, passing `pattern` through unchanged.
    pub fn not_like(col: C, pattern: impl Into<String>) -> Self {
        Self::NotLike(col, pattern.into())
    }

    /// `col LIKE 'value%'`; `%`, `_`, and `!` inside `value` are matched
    /// literally.
    pub fn starts_with(col: C, value: impl Into<String>) -> Self {
        Self::StartsWith(col, value.into())
    }

    /// `col LIKE '%value'`; `%`, `_`, and `!` inside `value` are matched
    /// literally.
    pub fn ends_with(col: C, value: impl Into<String>) -> Self {
        Self::EndsWith(col, value.into())
    }

    /// `col LIKE '%value%'`; `%`, `_`, and `!` inside `value` are matched
    /// literally.
    pub fn contains(col: C, value: impl Into<String>) -> Self {
        Self::Contains(col, value.into())
    }

    /// `col IN (…)`.
    pub fn one_of<I, V>(col: C, values: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        Self::In(col, values.into_iter().map(Into::into).collect())
    }

    /// `col NOT IN (…)`.
    pub fn not_one_of<I, V>(col: C, values: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        Self::NotIn(col, values.into_iter().map(Into::into).collect())
    }

    /// `col IS NULL`.
    pub fn is_null(col: C) -> Self {
        Self::IsNull(col)
    }

    /// `col IS NOT NULL`.
    pub fn is_not_null(col: C) -> Self {
        Self::IsNotNull(col)
    }

    /// Combines `predicates` with `AND`.
    pub fn and(predicates: impl IntoIterator<Item = Self>) -> Self {
        Self::And(predicates.into_iter().collect())
    }

    /// Combines `predicates` with `OR`.
    pub fn or(predicates: impl IntoIterator<Item = Self>) -> Self {
        Self::Or(predicates.into_iter().collect())
    }

    /// Negates this predicate.
    pub fn negate(self) -> Self {
        Self::Not(Box::new(self))
    }

    /// Renders the predicate onto `steps`, quoting column names with the
    /// driver's syntax and binding every value.
    pub(crate) fn render<DB: DriverOps>(&self, steps: &mut Vec<Step>) {
        match self {
            Self::Eq(col, value) => comparison::<DB, C>(steps, col, "=", value),
            Self::Ne(col, value) => comparison::<DB, C>(steps, col, "<>", value),
            Self::Lt(col, value) => comparison::<DB, C>(steps, col, "<", value),
            Self::Le(col, value) => comparison::<DB, C>(steps, col, "<=", value),
            Self::Gt(col, value) => comparison::<DB, C>(steps, col, ">", value),
            Self::Ge(col, value) => comparison::<DB, C>(steps, col, ">=", value),
            Self::Like(col, pattern) => pattern_step::<DB, C>(steps, col, pattern, false),
            Self::NotLike(col, pattern) => pattern_step::<DB, C>(steps, col, pattern, true),
            Self::StartsWith(col, value) => {
                let pattern = format!("{}%", escape_like(value));
                pattern_step::<DB, C>(steps, col, &pattern, false);
                steps.push(Step::Text(format!(" ESCAPE '{LIKE_ESCAPE}'")));
            }
            Self::EndsWith(col, value) => {
                let pattern = format!("%{}", escape_like(value));
                pattern_step::<DB, C>(steps, col, &pattern, false);
                steps.push(Step::Text(format!(" ESCAPE '{LIKE_ESCAPE}'")));
            }
            Self::Contains(col, value) => {
                let pattern = format!("%{}%", escape_like(value));
                pattern_step::<DB, C>(steps, col, &pattern, false);
                steps.push(Step::Text(format!(" ESCAPE '{LIKE_ESCAPE}'")));
            }
            Self::In(col, values) => list::<DB, C>(steps, col, values, "IN"),
            Self::NotIn(col, values) => list::<DB, C>(steps, col, values, "NOT IN"),
            Self::IsNull(col) => {
                steps.push(Step::Text(format!(
                    "{} IS NULL",
                    DB::quote_ident(col.name())
                )));
            }
            Self::IsNotNull(col) => {
                steps.push(Step::Text(format!(
                    "{} IS NOT NULL",
                    DB::quote_ident(col.name())
                )));
            }
            Self::And(children) => children_steps::<DB, C>(steps, children, " AND ", "1 = 1"),
            Self::Or(children) => children_steps::<DB, C>(steps, children, " OR ", "1 = 0"),
            Self::Not(inner) => {
                steps.push(Step::Text("NOT (".to_owned()));
                inner.render::<DB>(steps);
                steps.push(Step::Text(")".to_owned()));
            }
        }
    }
}

/// Renders `col <op> <bind>`.
fn comparison<DB: DriverOps, C: Column>(steps: &mut Vec<Step>, col: &C, op: &str, value: &Value) {
    steps.push(Step::Text(format!("{} {op} ", DB::quote_ident(col.name()))));
    steps.push(Step::Bind(value.clone()));
}

/// Renders `col [NOT] LIKE <bind>`.
fn pattern_step<DB: DriverOps, C: Column>(
    steps: &mut Vec<Step>,
    col: &C,
    pattern: &str,
    negated: bool,
) {
    let operator = if negated { "NOT LIKE" } else { "LIKE" };
    steps.push(Step::Text(format!(
        "{} {operator} ",
        DB::quote_ident(col.name())
    )));
    steps.push(Step::Bind(Value::Text(pattern.to_owned())));
}

/// Renders `col [NOT] IN (…)`, degrading to a constant for empty lists.
fn list<DB: DriverOps, C: Column>(steps: &mut Vec<Step>, col: &C, values: &[Value], op: &str) {
    if values.is_empty() {
        let constant = if op == "IN" { "1 = 0" } else { "1 = 1" };
        steps.push(Step::Text(constant.to_owned()));
        return;
    }
    steps.push(Step::Text(format!(
        "{} {op} (",
        DB::quote_ident(col.name())
    )));
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            steps.push(Step::Text(", ".to_owned()));
        }
        steps.push(Step::Bind(value.clone()));
    }
    steps.push(Step::Text(")".to_owned()));
}

/// Renders children separated by `separator`, degrading to `empty` when there
/// are none.
fn children_steps<DB: DriverOps, C: Column>(
    steps: &mut Vec<Step>,
    children: &[Predicate<C>],
    separator: &str,
    empty: &str,
) {
    if children.is_empty() {
        steps.push(Step::Text(empty.to_owned()));
        return;
    }
    steps.push(Step::Text("(".to_owned()));
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            steps.push(Step::Text(separator.to_owned()));
        }
        child.render::<DB>(steps);
    }
    steps.push(Step::Text(")".to_owned()));
}

/// Escapes the `LIKE` metacharacters (and the escape character itself) so the
/// value matches literally under `ESCAPE '!'`.
fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch == LIKE_ESCAPE || ch == '%' || ch == '_' {
            escaped.push(LIKE_ESCAPE);
        }
        escaped.push(ch);
    }
    escaped
}
