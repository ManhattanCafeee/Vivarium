# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0 — 2026-09-10

### Breaking

- Upgraded to `sqlx` 0.9 (MSRV 1.85 → Rust 1.94).
- Primary keys are typed: every CRUD helper takes (or returns) `T::Id`
  instead of `i64`, and `create` returns `T::Id`.
- The embedded example migrator is gone: `vivarium_db::MIGRATOR` and the
  `migrations/` directory were removed (moved to `examples/migrations/`).
  Applications own their migrations.
- `Query::first()` no longer appends `LIMIT 1` after the query's own
  `LIMIT`/`OFFSET` (invalid SQL); it now ignores them and emits
  `ORDER BY … LIMIT 1`.
- `Predicate`-based filtering replaces the `where_eq`-only surface;
  `Entity::columns_and_values` failures surface as `sqlx::Error::Encode`.

### Added

- `Predicate<C>`: a typed filter AST (`eq/ne/lt/le/gt/ge/like/not_like/
  starts_with/ends_with/contains/one_of/not_one_of/is_null/is_not_null/
  and/or/negate`), with `Query::filter`. `starts_with`/`ends_with`/
  `contains` escape `%`/`_` and use `ESCAPE '!'` so values match literally.
- `Query::select(&[C])` projection, `Query::paginate_with_total`, and
  `Query::raw_where(RawFragment)` — the explicit escape hatch, whose
  placeholder/bind counts are checked at construction.
- `Update::<T, C>` for partial-column updates, with `Expr::Now`
  (`CURRENT_TIMESTAMP`).
- `with_transaction(pool, async |tx| { … })`.
- MySQL is covered by an integration test (`MYSQL_DATABASE_URL`), and the
  `mysql` feature enables `sqlx/mysql-rsa` for non-TLS servers.
- `Value::TypedNull`, `Value::DateTime`/`NaiveDate`, and `Value::Uuid` are
  bound driver-correctly.

### Fixed

- `first()` produced invalid SQL when a limit or offset was set.
- Encoded values that fail (an `#[entity(json)]` field, for instance) are
  errors, not panics.
