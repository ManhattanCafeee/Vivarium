# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.1 — 2026-09-10

### Changed

- Re-exports follow the 0.3.1 crates (`vivarium-core`/`-macros`/`-db`/`-web`/
  `-config`). No API changes in the facade itself.

## 0.3.0 — 2026-09-10

### Breaking

- `MIGRATOR` is no longer re-exported (`vivarium-db` removed it): apply your
  own migrations with `sqlx::migrate!`.
- Re-exports follow the 0.3 APIs: CRUD helpers are typed by `Entity::Id`,
  `Page`/`Pagination` use the `items`/`per_page` field names, and
  `ApiError`/`ApiResponse` replaced the old error enum.
- The authentication surface follows `vivarium-web` 0.3: sessions and refresh
  tokens are stored as SHA-256 digests, `SessionAuth::new` takes
  `CookieOptions` and an absolute TTL, `RefreshTokenManager::rotate` takes a
  rebuild callback, and `cache::Cache` became `cache::CacheControl`.
- MSRV 1.85 → 1.94 (sqlx 0.9).

### Added

- Re-exports for the new surface: `Predicate`, `Update`, `Expr`,
  `with_transaction`, `RawFragment`, `RawFragmentError`, `PrimaryKey`,
  `NullType`, `EncodeError`, `ConfigOptions`, `ConfigWatcher`, `HandlerId`,
  `ApiResponse`, `ErrorKind`, `Texts`, `install_texts`, `ValidationErrors`,
  `FieldViolation`, the `openapi` module, and the `Garde*` extractors.
- Facade features `utoipa`, `utoipa-ui`, `validation-garde`, and `telemetry`;
  the `db` feature also enables `vivarium-web/sqlx`.
- Re-exports of the 0.3 authentication surface: `Hash`-free helpers
  (`hash_token`, `hash_with`, `needs_rehash`, `verify_and_upgrade`),
  `Argon2Params`, `VerifyOutcome`, `AccessClaims`, `Claims`, `SessionId`,
  `CookieOptions`, `SameSite`, `CacheControl`, `JwtConfig`, `JwtVerifier`,
  `KeyRing`, `should_extend`, `serve_with_shutdown`, and `shutdown_signal`.

### Documentation

- The quick-start example now reflects the envelope, `validator`-based
  extractors, and application-owned migrations; the end-to-end test pins the
  rendered JSON body.
