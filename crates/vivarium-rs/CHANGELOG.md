# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0 — unreleased

### Breaking

- `MIGRATOR` is no longer re-exported (`vivarium-db` removed it): apply your
  own migrations with `sqlx::migrate!`.
- Re-exports follow the 0.3 APIs: CRUD helpers are typed by `Entity::Id`,
  `Page`/`Pagination` use the `items`/`per_page` field names, and
  `ApiError`/`ApiResponse` replaced the old error enum.
- MSRV 1.85 → 1.94 (sqlx 0.9).

### Added

- Re-exports for the new surface: `Predicate`, `Update`, `Expr`,
  `with_transaction`, `RawFragment`, `RawFragmentError`, `PrimaryKey`,
  `NullType`, `EncodeError`, `ConfigOptions`, `ConfigWatcher`, `HandlerId`,
  `ApiResponse`, `ErrorKind`, `Texts`, `install_texts`, `ValidationErrors`,
  `FieldViolation`, the `openapi` module, and the `Garde*` extractors.
- Facade features `utoipa`, `utoipa-ui`, and `validation-garde`; the `db`
  feature also enables `vivarium-web/sqlx`.

### Documentation

- The quick-start example now reflects the envelope, `validator`-based
  extractors, and application-owned migrations; the end-to-end test pins the
  rendered JSON body.
