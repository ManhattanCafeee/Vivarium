# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.4.0 — 2026-09-29

### Added

- Re-exports of `Secret`, `Ttl`, `Digest`, `SecretError`, `TtlError`,
  `DigestError`, `SessionIdError` and `Argon2ParamsError` from
  `vivarium-web`.
- Re-exports of `Message`/`MessageError` (`vivarium-web`) and
  `PageNumber`/`PageSize` (`vivarium-core`).
- Re-exports of `CookieOptionsError` (`vivarium-web`) and `openapi::InfoError`
  (feature `utoipa`).

### Breaking

- The re-exported authentication surface follows `vivarium-web`'s checked
  construction: `KeyRing`, `sign_token`, `decode_token` and `jwt_auth` take a
  `Secret`, `RefreshTokenManager::new` / `SessionAuth::new` take `Ttl` values,
  and `SessionStore` / `RefreshTokenStore` take `&Digest`. See
  `vivarium-web`'s changelog for the full list.
- The re-exported pagination and message types follow their crates:
  `Pagination`/`Page` carry `PageNumber`/`PageSize`, `Texts` fields and
  `ApiResponse::message` are `Message`, and `ApiResponse::error`/
  `error_with_errors` take a `http::StatusCode` plus a `Message`. See the
  `vivarium-core` and `vivarium-web` changelogs.
- The re-exported db, web and config surfaces follow their crates:
  `RawFragmentError` is an enum, unset primary keys and blank raw fragments /
  `LIKE` needles are rejected, `CookieOptions`' fields are private (build with
  `CookieOptions::try_new` and the `with_*` builders; `CookieOptions::new` is
  gone), `openapi::info` returns a `Result` and `session_cookie_scheme` takes
  `&CookieOptions`, and `ConfigError` gained the four empty-input variants. See
  the `vivarium-db`, `vivarium-web` and `vivarium-config` changelogs.
- `#[derive(Entity)]` now rejects a blank `table`/`rename` literal and
  duplicate column names at compile time, so code that used to compile but
  produced `SELECT * FROM ""` or `INSERT INTO t ("id", "id") …` no longer
  builds. See the `vivarium-macros` changelog.

## 0.3.4 — 2026-09-14

### Changed

- Re-exports follow the 0.3.4 crates (`vivarium-core`/`-macros`/`-db`/`-web`/
  `-config`), including `vivarium-db`'s `with_transaction` `Send` fix. No API
  changes in the facade itself.

## 0.3.2 — 2026-09-11

### Changed

- Re-exports follow the 0.3.2 crates (`vivarium-core`/`-macros`/`-db`/`-web`/
  `-config`). No API changes in the facade itself.

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
