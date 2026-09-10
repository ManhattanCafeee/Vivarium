# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0 — unreleased

### Breaking

- The id field may now be `i64`, `u64`, `i32`, `u32`, or `String` (anything
  implementing `PrimaryKey`); other types are a compile error.
- Generated `columns_and_values` returns
  `Result<Vec<(&'static str, Value)>, EncodeError>`.
- Unknown field attributes are rejected instead of being silently ignored.

### Added

- `Option<T>` fields bind as typed NULLs
  (`Value::TypedNull(NullType::…)`).
- `chrono::DateTime<Utc>`, `chrono::NaiveDate`, and `uuid::Uuid` fields are
  supported (with the matching `vivarium-core` feature).
- Unknown-key diagnostics list the accepted keys; unsupported field types
  list the supported ones.

### Fixed

- `#[entity(json)]` no longer panics when a value fails to serialize: the
  failure is reported as `EncodeError` (surfaced by `vivarium-db` as
  `sqlx::Error::Encode`).
- Attribute parsing no longer swallows parse errors (`let _ = …`); a
  malformed `#[entity(...)]` is reported at its span.

### Tests

- UI cases: `id_not_primarykey` (was `id_not_i64`), `unknown_field_attr`;
  new pass case covering `u64`/`String` keys, typed NULLs, and JSON fields.
