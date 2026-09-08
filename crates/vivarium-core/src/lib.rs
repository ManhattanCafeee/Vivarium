//! # vivarium-core
//!
//! Core value types for the [`vivarium-rs`] family: pagination, ordering, bindable
//! column values, and the [`Entity`] contract shared by [`vivarium-db`] and
//! [`vivarium-web`].
//!
//! This crate has no database or HTTP dependencies; it is the shared
//! vocabulary of the family.
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs
//! [`vivarium-db`]: https://docs.rs/vivarium-db
//! [`vivarium-web`]: https://docs.rs/vivarium-web
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::fmt;

/// A page request: 1-based page number plus page size.
///
/// Construction normalizes out-of-range values (see [`normalize`]).
///
/// [`normalize`]: Pagination::normalize
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pagination {
    /// 1-based page number, normalized to `1..=MAX_PAGE`.
    pub page: u32,
    /// Number of rows per page, normalized to `1..=MAX_SIZE`.
    pub size: u32,
}

impl Pagination {
    /// Maximum accepted page number.
    pub const MAX_PAGE: u32 = 1_000_000;
    /// Maximum accepted page size.
    pub const MAX_SIZE: u32 = 100;
    /// Page size used when a non-positive size is requested.
    pub const DEFAULT_SIZE: u32 = 20;

    /// Constructs a pagination, normalizing out-of-range values in place.
    ///
    /// `page` is clamped to `1..=MAX_PAGE`; `size` to `1..=MAX_SIZE`, with
    /// `0` mapped to [`DEFAULT_SIZE`].
    ///
    /// [`DEFAULT_SIZE`]: Pagination::DEFAULT_SIZE
    pub fn new(page: u32, size: u32) -> Self {
        let mut pagination = Self { page, size };
        pagination.normalize();
        pagination
    }

    /// Normalizes `page` and `size` in place.
    ///
    /// - `page == 0` becomes `1`; values above [`MAX_PAGE`] clamp down.
    /// - `size == 0` becomes [`DEFAULT_SIZE`]; values above [`MAX_SIZE`] clamp
    ///   down.
    ///
    /// [`MAX_PAGE`]: Pagination::MAX_PAGE
    /// [`MAX_SIZE`]: Pagination::MAX_SIZE
    /// [`DEFAULT_SIZE`]: Pagination::DEFAULT_SIZE
    pub fn normalize(&mut self) {
        if self.page == 0 {
            self.page = 1;
        }
        if self.page > Self::MAX_PAGE {
            self.page = Self::MAX_PAGE;
        }
        if self.size == 0 {
            self.size = Self::DEFAULT_SIZE;
        }
        if self.size > Self::MAX_SIZE {
            self.size = Self::MAX_SIZE;
        }
    }

    /// Returns the SQL `(LIMIT, OFFSET)` pair for this page.
    ///
    /// `LIMIT` is `size`; `OFFSET` is `(page - 1) * size`.
    pub fn limit_offset(&self) -> (u64, u64) {
        (
            u64::from(self.size),
            u64::from(self.page - 1) * u64::from(self.size),
        )
    }
}

impl Default for Pagination {
    /// A pagination for the first page of [`DEFAULT_SIZE`] rows.
    ///
    /// [`DEFAULT_SIZE`]: Pagination::DEFAULT_SIZE
    fn default() -> Self {
        Self::new(1, Self::DEFAULT_SIZE)
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Order {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

impl fmt::Display for Order {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Order::Asc => "ASC",
            Order::Desc => "DESC",
        })
    }
}

/// The name of a table column.
///
/// Columns form a closed set: implement this on your own enum (or unit
/// structs), never on strings. Query builders only accept types implementing
/// [`Column`], which makes column-name injection impossible at compile time —
/// an upgrade over the runtime whitelist checks of the Go reference
/// implementation.
///
/// ```
/// use vivarium_core::Column;
///
/// #[derive(Clone, Copy)]
/// enum UserCol {
///     Id,
///     Name,
///     Age,
/// }
///
/// impl Column for UserCol {
///     fn name(&self) -> &'static str {
///         match self {
///             UserCol::Id => "id",
///             UserCol::Name => "name",
///             UserCol::Age => "age",
///         }
///     }
/// }
/// ```
pub trait Column {
    /// The column name in the database.
    fn name(&self) -> &'static str;
}

/// A sort request: a [`Column`] plus an [`Order`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sorter<C: Column> {
    /// The column to sort by.
    pub col: C,
    /// The sort direction.
    pub order: Order,
}

impl<C: Column> Sorter<C> {
    /// Constructs a sorter.
    pub fn new(col: C, order: Order) -> Self {
        Self { col, order }
    }
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// Total number of matching rows across all pages.
    pub total: u64,
    /// The (normalized) page number this page represents.
    pub page: u32,
    /// The (normalized) page size.
    pub size: u32,
    /// The rows of this page.
    pub content: Vec<T>,
}

impl<T> Page<T> {
    /// Total number of pages, rounding up.
    ///
    /// Returns `0` when `size` is `0` (only possible through manual
    /// construction).
    pub fn pages(&self) -> u64 {
        if self.size == 0 {
            0
        } else {
            self.total.div_ceil(u64::from(self.size))
        }
    }
}

/// A bindable column value, used by the CRUD helpers of `vivarium-db`.
///
/// `#[derive(Entity)]` maps common field types onto these variants
/// automatically: integers onto [`I64`], floats onto [`F64`], `String` onto
/// [`Text`], `Vec<u8>` onto [`Bytes`], `bool` onto [`Bool`], and
/// `serde_json::Value` onto [`Json`]. `Option<T>` maps to [`Null`] when
/// `None`.
///
/// [`I64`]: Value::I64
/// [`F64`]: Value::F64
/// [`Text`]: Value::Text
/// [`Bytes`]: Value::Bytes
/// [`Bool`]: Value::Bool
/// [`Json`]: Value::Json
/// [`Null`]: Value::Null
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// A 64-bit integer column.
    I64(i64),
    /// A 64-bit float column.
    F64(f64),
    /// A text column.
    Text(String),
    /// A binary column.
    Bytes(Vec<u8>),
    /// A boolean column.
    Bool(bool),
    /// A JSON column (`TEXT` on SQLite, `JSONB` on PostgreSQL, `JSON` on
    /// MySQL).
    Json(serde_json::Value),
}

/// A type mappable to a single database table, enabling the generic CRUD and
/// query helpers of `vivarium-db`.
///
/// Usually derived: see [`vivarium_macros::Entity`]. A manual implementation
/// requires the table name, the id column name, the id value, and the
/// non-id column/value pairs in insert order.
///
/// [`vivarium_macros::Entity`]: https://docs.rs/vivarium-macros/latest/vivarium_macros/derive.Entity.html
///
/// ```
/// use vivarium_core::{Entity, Value};
///
/// struct Book {
///     id: i64,
///     title: String,
/// }
///
/// impl Entity for Book {
///     const TABLE: &'static str = "books";
///     const ID_COLUMN: &'static str = "id";
///
///     fn id(&self) -> i64 {
///         self.id
///     }
///
///     fn columns_and_values(&self) -> Vec<(&'static str, Value)> {
///         vec![("title", Value::Text(self.title.clone()))]
///     }
/// }
/// ```
pub trait Entity {
    /// The table name.
    const TABLE: &'static str;
    /// The primary-key column name.
    const ID_COLUMN: &'static str;
    /// The primary-key value.
    fn id(&self) -> i64;
    /// All non-id columns and their bindable values, in declaration order.
    ///
    /// This is what `create` and `update_by_id` insert/update; the id column
    /// is handled separately (inserted when `id()` is non-zero, used as the
    /// `WHERE` key otherwise).
    fn columns_and_values(&self) -> Vec<(&'static str, Value)>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_normalizes_page() {
        assert_eq!(Pagination::new(0, 20).page, 1);
        assert_eq!(Pagination::new(1, 20).page, 1);
        assert_eq!(Pagination::new(1_000_001, 20).page, 1_000_000);
    }

    #[test]
    fn pagination_normalizes_size() {
        assert_eq!(Pagination::new(1, 0).size, 20);
        assert_eq!(Pagination::new(1, 20).size, 20);
        assert_eq!(Pagination::new(1, 101).size, 100);
    }

    #[test]
    fn pagination_limit_offset_math() {
        assert_eq!(Pagination::new(1, 10).limit_offset(), (10, 0));
        assert_eq!(Pagination::new(3, 10).limit_offset(), (10, 20));
        assert_eq!(
            Pagination::new(1_000_000, 100).limit_offset(),
            (100, 99_999_900)
        );
    }

    #[test]
    fn page_counts_pages_rounding_up() {
        let page: Page<()> = Page {
            total: 45,
            page: 1,
            size: 10,
            content: vec![],
        };
        assert_eq!(page.pages(), 5);
        let exact: Page<()> = Page {
            total: 40,
            page: 1,
            size: 10,
            content: vec![],
        };
        assert_eq!(exact.pages(), 4);
        let empty: Page<()> = Page {
            total: 0,
            page: 1,
            size: 10,
            content: vec![],
        };
        assert_eq!(empty.pages(), 0);
    }

    #[test]
    fn default_pagination_is_first_page() {
        let p = Pagination::default();
        assert_eq!((p.page, p.size), (1, 20));
    }

    #[test]
    fn order_displays_sql_keywords() {
        assert_eq!(Order::Asc.to_string(), "ASC");
        assert_eq!(Order::Desc.to_string(), "DESC");
    }
}
