//! # vivarium-core
//!
//! Core value types for the [`vivarium-rs`] family: pagination, ordering, bindable
//! column values, and the [`Entity`] contract shared by [`vivarium-db`] and
//! [`vivarium-web`].
//!
//! This crate has no database or HTTP dependencies; it is the shared
//! vocabulary of the family.
//!
//! # Features
//!
//! - `chrono` — adds [`Value::DateTime`] / [`Value::NaiveDate`] and the
//!   matching [`NullType`] variants.
//! - `uuid` — adds the [`Value::Uuid`] variant.
//! - `utoipa` — derives `ToSchema` for [`Page`] / [`Pagination`] and
//!   implements `IntoParams` for [`Pagination`]. The impls must live in this
//!   crate because a trait impl shares a crate with its type.
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs
//! [`vivarium-db`]: https://docs.rs/vivarium-db
//! [`vivarium-web`]: https://docs.rs/vivarium-web
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A page request: 1-based page number plus page size.
///
/// Construction normalizes out-of-range values (see [`normalize`]).
///
/// [`normalize`]: Pagination::normalize
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(from = "PaginationArgs")]
pub struct Pagination {
    /// 1-based page number, normalized to `1..=MAX_PAGE`.
    pub page: u32,
    /// Number of rows per page, normalized to `1..=MAX_PER_PAGE`.
    pub per_page: u32,
}

impl Pagination {
    /// Maximum accepted page number.
    pub const MAX_PAGE: u32 = 1_000_000;
    /// Maximum accepted page size.
    pub const MAX_PER_PAGE: u32 = 100;
    /// Page size used when a non-positive size is requested.
    pub const DEFAULT_PER_PAGE: u32 = 20;

    /// Constructs a pagination, normalizing out-of-range values in place.
    ///
    /// `page` is clamped to `1..=MAX_PAGE`; `per_page` to
    /// `1..=MAX_PER_PAGE`, with `0` mapped to [`DEFAULT_PER_PAGE`].
    ///
    /// [`DEFAULT_PER_PAGE`]: Pagination::DEFAULT_PER_PAGE
    pub fn new(page: u32, per_page: u32) -> Self {
        let mut pagination = Self { page, per_page };
        pagination.normalize();
        pagination
    }

    /// Normalizes `page` and `per_page` in place.
    ///
    /// - `page == 0` becomes `1`; values above [`MAX_PAGE`] clamp down.
    /// - `per_page == 0` becomes [`DEFAULT_PER_PAGE`]; values above
    ///   [`MAX_PER_PAGE`] clamp down.
    ///
    /// [`MAX_PAGE`]: Pagination::MAX_PAGE
    /// [`MAX_PER_PAGE`]: Pagination::MAX_PER_PAGE
    /// [`DEFAULT_PER_PAGE`]: Pagination::DEFAULT_PER_PAGE
    pub fn normalize(&mut self) {
        if self.page == 0 {
            self.page = 1;
        }
        if self.page > Self::MAX_PAGE {
            self.page = Self::MAX_PAGE;
        }
        if self.per_page == 0 {
            self.per_page = Self::DEFAULT_PER_PAGE;
        }
        if self.per_page > Self::MAX_PER_PAGE {
            self.per_page = Self::MAX_PER_PAGE;
        }
    }

    /// Returns the SQL `(LIMIT, OFFSET)` pair for this page.
    ///
    /// `LIMIT` is `per_page`; `OFFSET` is `(page - 1) * per_page`.
    pub fn limit_offset(&self) -> (u64, u64) {
        (
            u64::from(self.per_page),
            u64::from(self.page - 1) * u64::from(self.per_page),
        )
    }
}

impl Default for Pagination {
    /// A pagination for the first page of [`DEFAULT_PER_PAGE`] rows.
    ///
    /// [`DEFAULT_PER_PAGE`]: Pagination::DEFAULT_PER_PAGE
    fn default() -> Self {
        Self::new(1, Self::DEFAULT_PER_PAGE)
    }
}

/// The deserialization shape of [`Pagination`].
///
/// Deserialization goes through [`Pagination::new`] so the normalization
/// invariant holds for every `Pagination` in existence — a wire value such as
/// `{"page": 0}` would otherwise reach [`Pagination::limit_offset`] with
/// `page == 0` and underflow. Missing fields take the documented defaults,
/// matching the `IntoParams` schema.
#[derive(Deserialize)]
#[serde(default)]
struct PaginationArgs {
    page: u32,
    per_page: u32,
}

impl Default for PaginationArgs {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: Pagination::DEFAULT_PER_PAGE,
        }
    }
}

impl From<PaginationArgs> for Pagination {
    fn from(args: PaginationArgs) -> Self {
        Self::new(args.page, args.per_page)
    }
}

#[cfg(feature = "utoipa")]
impl utoipa::IntoParams for Pagination {
    fn into_params(
        parameter_in_provider: impl Fn() -> Option<utoipa::openapi::path::ParameterIn>,
    ) -> Vec<utoipa::openapi::path::Parameter> {
        use utoipa::openapi::Required;
        use utoipa::openapi::path::{ParameterBuilder, ParameterIn};
        use utoipa::openapi::schema::{ObjectBuilder, Type};

        let location = || parameter_in_provider().unwrap_or(ParameterIn::Query);

        let page = ParameterBuilder::new()
            .name("page")
            .parameter_in(location())
            .required(Required::False)
            .schema(Some(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(1_usize))
                    .maximum(Some(Pagination::MAX_PAGE as usize))
                    .default(Some(serde_json::json!(1))),
            ))
            .build();

        let per_page = ParameterBuilder::new()
            .name("per_page")
            .parameter_in(location())
            .required(Required::False)
            .schema(Some(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(1_usize))
                    .maximum(Some(Pagination::MAX_PER_PAGE as usize))
                    .default(Some(serde_json::json!(Pagination::DEFAULT_PER_PAGE))),
            ))
            .build();

        vec![page, per_page]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Page<T> {
    /// The rows of this page.
    pub items: Vec<T>,
    /// Total number of matching rows across all pages.
    pub total: u64,
    /// The (normalized) page number this page represents.
    pub page: u32,
    /// The (normalized) page size.
    pub per_page: u32,
}

impl<T> Page<T> {
    /// Total number of pages, rounding up.
    ///
    /// Returns `0` when `per_page` is `0` (only possible through manual
    /// construction).
    pub fn pages(&self) -> u64 {
        if self.per_page == 0 {
            0
        } else {
            self.total.div_ceil(u64::from(self.per_page))
        }
    }
}

/// A bindable column value, used by the CRUD helpers of `vivarium-db`.
///
/// `#[derive(Entity)]` maps common field types onto these variants
/// automatically: integers onto [`I64`], floats onto [`F64`], `String` onto
/// [`Text`], `Vec<u8>` onto [`Bytes`], `bool` onto [`Bool`], and
/// `serde_json::Value` onto [`Json`]. `Option<T>` maps to [`TypedNull`] with
/// the matching [`NullType`], so drivers that type-check parameters
/// (PostgreSQL) accept the comparison.
///
/// The blanket `From` impls convert integers with `as i64`, so `u64`/`usize`
/// above `i64::MAX` wrap; primary keys go through
/// [`PrimaryKey::into_value`] instead, which returns an error.
///
/// [`I64`]: Value::I64
/// [`F64`]: Value::F64
/// [`Text`]: Value::Text
/// [`Bytes`]: Value::Bytes
/// [`Bool`]: Value::Bool
/// [`Json`]: Value::Json
/// [`TypedNull`]: Value::TypedNull
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL `NULL`, bound as an `INT8`-typed NULL.
    ///
    /// PostgreSQL rejects `col = $1` with this against non-integer columns at
    /// prepare time; prefer [`Value::TypedNull`] when the column type is
    /// known.
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
    /// A `NULL` with an explicit SQL type (see [`NullType`]).
    TypedNull(NullType),
    /// A timestamp column (requires the `chrono` feature); binds to
    /// PostgreSQL `TIMESTAMPTZ`, SQLite text, and MySQL `TIMESTAMP` (the
    /// driver's `compatible` check also accepts `DATETIME` columns).
    #[cfg(feature = "chrono")]
    DateTime(chrono::DateTime<chrono::Utc>),
    /// A date column without a time component (requires the `chrono`
    /// feature).
    #[cfg(feature = "chrono")]
    NaiveDate(chrono::NaiveDate),
    /// A UUID column (requires the `uuid` feature).
    #[cfg(feature = "uuid")]
    Uuid(uuid::Uuid),
}

/// The SQL type a [`Value::TypedNull`] binds as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NullType {
    /// A 64-bit integer column.
    I64,
    /// A 64-bit float column.
    F64,
    /// A boolean column.
    Bool,
    /// A text column.
    Text,
    /// A binary column.
    Bytes,
    /// A JSON column.
    Json,
    /// A timestamp column (requires the `chrono` feature).
    #[cfg(feature = "chrono")]
    DateTime,
    /// A date column (requires the `chrono` feature).
    #[cfg(feature = "chrono")]
    NaiveDate,
    /// A UUID column (requires the `uuid` feature).
    #[cfg(feature = "uuid")]
    Uuid,
}

impl From<i8> for Value {
    fn from(value: i8) -> Self {
        Value::I64(i64::from(value))
    }
}

impl From<i16> for Value {
    fn from(value: i16) -> Self {
        Value::I64(i64::from(value))
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::I64(i64::from(value))
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::I64(value)
    }
}

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Value::I64(i64::from(value))
    }
}

impl From<u16> for Value {
    fn from(value: u16) -> Self {
        Value::I64(i64::from(value))
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value::I64(i64::from(value))
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Value::I64(value as i64)
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Value::I64(value as i64)
    }
}

impl From<isize> for Value {
    fn from(value: isize) -> Self {
        Value::I64(value as i64)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::F64(f64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::F64(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_owned())
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Value::Text(value.clone())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Value::Bytes(value)
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Value::Bytes(value.to_vec())
    }
}

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        Value::Json(value)
    }
}

#[cfg(feature = "chrono")]
impl From<chrono::DateTime<chrono::Utc>> for Value {
    fn from(value: chrono::DateTime<chrono::Utc>) -> Self {
        Value::DateTime(value)
    }
}

#[cfg(feature = "chrono")]
impl From<chrono::NaiveDate> for Value {
    fn from(value: chrono::NaiveDate) -> Self {
        Value::NaiveDate(value)
    }
}

#[cfg(feature = "uuid")]
impl From<uuid::Uuid> for Value {
    fn from(value: uuid::Uuid) -> Self {
        Value::Uuid(value)
    }
}

/// Converts an optional value: `Some` converts the inner value, `None`
/// becomes [`Value::Null`]. Nested `Option`s collapse — `Some(None)` and
/// `None` are both [`Value::Null`] — while `#[derive(Entity)]` rejects
/// nested `Option` fields at compile time.
impl<T> From<Option<T>> for Value
where
    T: Into<Value>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Value::Null,
        }
    }
}

/// The error returned when an entity field cannot be encoded into a
/// [`Value`].
///
/// Produced by the generated `columns_and_values` of `#[derive(Entity)]` —
/// most notably by an `#[entity(json)]` field whose `Serialize` fails. The db
/// layer converts it into `sqlx::Error::Encode`.
#[derive(Debug)]
pub struct EncodeError {
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl EncodeError {
    /// Constructs an error from a description.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Constructs an error from a description and an underlying cause.
    pub fn with_source(
        message: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(source.into()),
        }
    }
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{}: {source}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for EncodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| &**source as &(dyn std::error::Error + 'static))
    }
}

impl From<serde_json::Error> for EncodeError {
    fn from(error: serde_json::Error) -> Self {
        Self::with_source("field marked #[entity(json)] failed to serialize", error)
    }
}

/// The error returned when a primary-key value cannot be represented as a
/// bindable [`Value`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryKeyError {
    message: Cow<'static, str>,
}

impl PrimaryKeyError {
    /// Constructs an error with a static description.
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PrimaryKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PrimaryKeyError {}

/// A primary-key type usable by the generic CRUD helpers of `vivarium-db`.
///
/// Implemented for `i64`, `u64`, `i32`, `u32`, and `String` (for
/// `VARCHAR`-keyed tables). Values bind through [`into_value`]; a `u64` above
/// `i64::MAX` returns an error instead of wrapping silently.
///
/// [`into_value`]: PrimaryKey::into_value
pub trait PrimaryKey: Clone + Eq + Send + Sync + 'static {
    /// The bindable value of this key.
    fn into_value(self) -> Result<Value, PrimaryKeyError>;

    /// Whether this key means "not set yet": `0` for integers, empty for
    /// `String`.
    fn is_unset(&self) -> bool;

    /// Converts a database-generated id into this key type.
    ///
    /// Used when the database assigns the key (autoincrement / serial).
    /// Integer keys reject values that do not fit; `String` keys are never
    /// generated by the database and always fail.
    fn from_generated(id: i64) -> Result<Self, PrimaryKeyError>;

    /// Whether the database can assign this key when an entity does not
    /// supply one.
    ///
    /// `true` for integer keys (autoincrement / serial); `false` for `String`
    /// keys, which always need an explicit value — callers check this *before*
    /// writing, so an unset key is rejected instead of leaving a row behind.
    fn is_database_generated() -> bool {
        true
    }
}

impl PrimaryKey for i64 {
    fn into_value(self) -> Result<Value, PrimaryKeyError> {
        Ok(Value::I64(self))
    }

    fn is_unset(&self) -> bool {
        *self == 0
    }

    fn from_generated(id: i64) -> Result<Self, PrimaryKeyError> {
        Ok(id)
    }
}

impl PrimaryKey for u64 {
    fn into_value(self) -> Result<Value, PrimaryKeyError> {
        i64::try_from(self)
            .map(Value::I64)
            .map_err(|_| PrimaryKeyError::new("primary key value exceeds i64::MAX"))
    }

    fn is_unset(&self) -> bool {
        *self == 0
    }

    fn from_generated(id: i64) -> Result<Self, PrimaryKeyError> {
        u64::try_from(id).map_err(|_| {
            PrimaryKeyError::new("database returned a negative id for a u64 primary key")
        })
    }
}

impl PrimaryKey for i32 {
    fn into_value(self) -> Result<Value, PrimaryKeyError> {
        Ok(Value::I64(i64::from(self)))
    }

    fn is_unset(&self) -> bool {
        *self == 0
    }

    fn from_generated(id: i64) -> Result<Self, PrimaryKeyError> {
        i32::try_from(id)
            .map_err(|_| PrimaryKeyError::new("database-generated id does not fit in i32"))
    }
}

impl PrimaryKey for u32 {
    fn into_value(self) -> Result<Value, PrimaryKeyError> {
        Ok(Value::I64(i64::from(self)))
    }

    fn is_unset(&self) -> bool {
        *self == 0
    }

    fn from_generated(id: i64) -> Result<Self, PrimaryKeyError> {
        u32::try_from(id)
            .map_err(|_| PrimaryKeyError::new("database-generated id does not fit in u32"))
    }
}

impl PrimaryKey for String {
    fn into_value(self) -> Result<Value, PrimaryKeyError> {
        Ok(Value::Text(self))
    }

    fn is_database_generated() -> bool {
        false
    }

    fn is_unset(&self) -> bool {
        self.is_empty()
    }

    fn from_generated(_id: i64) -> Result<Self, PrimaryKeyError> {
        Err(PrimaryKeyError::new(
            "VARCHAR primary keys cannot be generated by the database",
        ))
    }
}

/// A type mappable to a single database table, enabling the generic CRUD and
/// query helpers of `vivarium-db`.
///
/// Usually derived: see [`vivarium_macros::Entity`]. A manual implementation
/// requires the table name, the id column name, the primary-key type, the id
/// value, and the non-id column/value pairs in insert order.
///
/// [`vivarium_macros::Entity`]: https://docs.rs/vivarium-macros/latest/vivarium_macros/derive.Entity.html
///
/// ```
/// use vivarium_core::{EncodeError, Entity, Value};
///
/// struct Book {
///     id: i64,
///     title: String,
/// }
///
/// impl Entity for Book {
///     type Id = i64;
///     const TABLE: &'static str = "books";
///     const ID_COLUMN: &'static str = "id";
///
///     fn id(&self) -> Self::Id {
///         self.id
///     }
///
///     fn columns_and_values(&self) -> Result<Vec<(&'static str, Value)>, EncodeError> {
///         Ok(vec![("title", Value::Text(self.title.clone()))])
///     }
/// }
/// ```
pub trait Entity {
    /// The primary-key type of this table.
    type Id: PrimaryKey;

    /// The table name.
    const TABLE: &'static str;

    /// The primary-key column name.
    const ID_COLUMN: &'static str;

    /// The primary-key value.
    fn id(&self) -> Self::Id;

    /// All non-id columns and their bindable values, in declaration order.
    ///
    /// This is what `create` and `update_by_id` insert/update; the id column
    /// is handled separately. Fails when a field cannot be encoded, for
    /// example an `#[entity(json)]` field whose `Serialize` errors.
    fn columns_and_values(&self) -> Result<Vec<(&'static str, Value)>, EncodeError>;
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
    fn pagination_normalizes_per_page() {
        assert_eq!(Pagination::new(1, 0).per_page, 20);
        assert_eq!(Pagination::new(1, 20).per_page, 20);
        assert_eq!(Pagination::new(1, 101).per_page, 100);
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
            items: vec![],
            total: 45,
            page: 1,
            per_page: 10,
        };
        assert_eq!(page.pages(), 5);

        let exact: Page<()> = Page {
            items: vec![],
            total: 40,
            page: 1,
            per_page: 10,
        };
        assert_eq!(exact.pages(), 4);

        let empty: Page<()> = Page {
            items: vec![],
            total: 0,
            page: 1,
            per_page: 10,
        };
        assert_eq!(empty.pages(), 0);

        let zero_sized: Page<()> = Page {
            items: vec![],
            total: 10,
            page: 1,
            per_page: 0,
        };
        assert_eq!(zero_sized.pages(), 0);
    }

    #[test]
    fn page_serializes_as_items_and_per_page() {
        let page = Page {
            items: vec![1_u32, 2],
            total: 2,
            page: 1,
            per_page: 10,
        };
        assert_eq!(
            serde_json::to_value(&page).expect("page serializes"),
            serde_json::json!({ "items": [1, 2], "total": 2, "page": 1, "per_page": 10 })
        );

        let pagination = Pagination::new(2, 5);
        assert_eq!(
            serde_json::to_value(pagination).expect("pagination serializes"),
            serde_json::json!({ "page": 2, "per_page": 5 })
        );
        let round_trip: Pagination =
            serde_json::from_value(serde_json::json!({ "page": 2, "per_page": 5 }))
                .expect("pagination deserializes");
        assert_eq!(round_trip, pagination);
    }

    #[test]
    fn pagination_deserialization_normalizes_and_defaults() {
        let defaulted: Pagination =
            serde_json::from_value(serde_json::json!({})).expect("empty object defaults");
        assert_eq!((defaulted.page, defaulted.per_page), (1, 20));

        let out_of_range: Pagination =
            serde_json::from_value(serde_json::json!({ "page": 0, "per_page": 0 }))
                .expect("zeros are normalized");
        assert_eq!((out_of_range.page, out_of_range.per_page), (1, 20));
        // The normalization invariant is what keeps this arithmetic safe.
        assert_eq!(out_of_range.limit_offset(), (20, 0));
    }

    #[test]
    fn default_pagination_is_first_page() {
        let p = Pagination::default();
        assert_eq!((p.page, p.per_page), (1, 20));
    }

    #[test]
    fn order_displays_sql_keywords() {
        assert_eq!(Order::Asc.to_string(), "ASC");
        assert_eq!(Order::Desc.to_string(), "DESC");
    }

    #[test]
    fn primary_key_binds_supported_types() {
        assert_eq!(7_i64.into_value().expect("i64 key"), Value::I64(7));
        assert_eq!(7_u64.into_value().expect("u64 key"), Value::I64(7));
        assert_eq!(7_i32.into_value().expect("i32 key"), Value::I64(7));
        assert_eq!(7_u32.into_value().expect("u32 key"), Value::I64(7));
        assert_eq!(
            String::from("abc").into_value().expect("string key"),
            Value::Text("abc".to_owned())
        );
    }

    #[test]
    fn primary_key_rejects_values_above_i64_max() {
        assert!(u64::MAX.into_value().is_err());
        assert_eq!(
            (i64::MAX as u64).into_value().expect("i64::MAX fits"),
            Value::I64(i64::MAX)
        );
    }

    #[test]
    fn primary_key_unset_semantics() {
        assert!(0_i64.is_unset());
        assert!(0_u32.is_unset());
        assert!(String::new().is_unset());
        assert!(!5_u64.is_unset());
        assert!(!String::from("x").is_unset());
    }

    #[test]
    fn encode_error_exposes_display_and_source() {
        let error = EncodeError::with_source(
            "field failed to serialize",
            serde_json::Error::io(std::io::Error::other("nope")),
        );
        assert!(error.to_string().contains("field failed to serialize"));
        assert!(std::error::Error::source(&error).is_some());

        let bare = EncodeError::new("plain");
        assert_eq!(bare.to_string(), "plain");
        assert!(std::error::Error::source(&bare).is_none());
    }

    #[cfg(feature = "utoipa")]
    #[test]
    fn schema_base_name_does_not_depend_on_the_type_argument() {
        use utoipa::ToSchema;

        // utoipa composes instantiation names as `<Base>_<Child>` at the
        // reference site, so the base name must stay free of type arguments.
        // The composed keys themselves are pinned by the web golden test.
        assert_eq!(Pagination::name(), "Pagination");
        assert_eq!(Page::<u32>::name(), "Page");
        assert_eq!(Page::<String>::name(), "Page");
    }

    #[cfg(feature = "utoipa")]
    #[test]
    fn pagination_into_params_describes_both_parameters() {
        use utoipa::IntoParams;

        let params = Pagination::into_params(|| None);
        let names = params
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["page", "per_page"]);
    }
}
