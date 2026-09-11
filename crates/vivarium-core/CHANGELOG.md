# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.2 — 2026-09-11

### Changed

- Version bump only, released in lockstep with the 0.3.2 workspace; no code
  changes.

## 0.3.1 — 2026-09-10

### Fixed

- `Pagination::limit_offset` no longer panics in debug or wraps in release for
  values that bypassed normalization: it normalizes internally (`page` 0 reads
  as 1, `per_page` 0 as `DEFAULT_PER_PAGE`) and saturates the offset
  arithmetic, so `Pagination { page: 0, per_page: 0 }.limit_offset()` is
  `(20, 0)` instead of `attempt to subtract with overflow`.

### Changed

- Docs: the private `PaginationArgs` note no longer claims the normalization
  invariant holds for every `Pagination` in existence, `limit_offset` documents
  that it is safe for unnormalized input, and the `Value` docs record that a
  `#[entity(json)]` field serializes `None` to JSON `null` rather than a typed
  SQL `NULL`.

## 0.3.0 — 2026-09-10

### Breaking

- `Entity` gained an associated primary-key type: `type Id: PrimaryKey`,
  `fn id(&self) -> Self::Id`, and `columns_and_values` is now fallible
  (`Result<Vec<(&'static str, Value)>, EncodeError>`). The `i64`-only
  restriction is gone.
- `Page` fields renamed for the wire contract: `content` → `items`,
  `size` → `per_page`.
- `Pagination::size` renamed to `per_page`, with `MAX_SIZE` →
  `MAX_PER_PAGE` and `DEFAULT_SIZE` → `DEFAULT_PER_PAGE`.
- `Option<T>` values from `#[derive(Entity)]` now bind as
  `Value::TypedNull(NullType::…)` instead of `Value::Null`.

### Added

- `PrimaryKey` (implemented for `i64`, `u64`, `i32`, `u32`, `String`) with
  `into_value`, `is_unset`, and `from_generated`; `u64` values above
  `i64::MAX` are rejected instead of wrapping.
- `Value::TypedNull(NullType)` plus the `NullType` enum for typed NULLs.
- Optional `chrono` feature (`Value::DateTime`, `Value::NaiveDate`) and
  `uuid` feature (`Value::Uuid`).
- Optional `utoipa` feature: `ToSchema` for `Page`/`Pagination` and
  `IntoParams` for `Pagination`. It lives here because a trait impl must
  share a crate with its type.
- `EncodeError`, `PrimaryKeyError`.
- `serde` derives on `Page`/`Pagination` (the declared `serde` dependency is
  now actually used).

### Fixed

- Typed NULLs are available through the derive, so the previous "use raw
  sqlx for typed NULLs" caveat no longer applies.
- Integer conversions that would wrap are documented, and primary keys
  report an error instead.
