# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0 — unreleased

### Breaking

- `ApiError` is a struct with private fields and one constructor per kind,
  not an enum: `ApiError::NotFound(msg)` → `ApiError::not_found(msg)`,
  `ApiError::Internal { system }` → `ApiError::internal(source)` (or
  `ApiError::database(source)`), and so on.
- `ApiError::code() -> &'static str` is gone; use `kind()`
  (`ErrorKind`, with `as_str()` for logs) and `status()`.
- `set_debug_mode` → `install_debug_mode` (installs once per process);
  `debug_mode()` still reports the flag.
- `ApiError::Validation(String)` → `ApiError::validation(ValidationErrors)`;
  validation detail is structured instead of a formatted string.
- A body that fails to deserialize now answers `400` (`DataParse`, message
  `"invalid request data"`), not `422`; path/query/form parse failures answer
  `400` (`BadRequest`). Semantic validation failures remain `422`.
- The default rejection messages are English constants. Applications
  localize them with [`install_texts`]; parser detail is only echoed when
  `Texts::echo_details` is enabled.
- `Varser<T>`/`QueryVarser`/`PathVarser`/`FormVarser` now bind
  `validator::Validate`; the garde equivalents moved behind the
  `validation-garde` feature (previously garde was the only backend and a
  mandatory dependency).

### Added

- `ApiResponse<T>`: the response envelope (`code`, `message`, optional
  `errors`, `data`), rendering through the same code path as `ApiError`.
- `ErrorKind` (nine kinds, `status()`, `as_str()`), `Result<T, E = ApiError>`,
  `ApiError::with_source`.
- `ValidationErrors` / `FieldViolation`, converted from
  `validator::ValidationErrors` (recursively flattened, `a.b[0].c`) and from
  `garde::Report`.
- `Texts` / `install_texts` / `texts` for message localization.
- Feature `sqlx`: `From<sqlx::Error>` and `ApiError::conflict_from_db`
  (unique violations → 409) without making sqlx a mandatory dependency.
- Features `utoipa` / `utoipa-ui` with `vivarium_web::openapi`:
  `session_cookie_scheme`, `bearer_scheme`, `info`, `mount` (Scalar +
  Swagger UI, assets vendored), and `ToSchema` derives for the envelope and
  validation types. Swagger UI assets are vendored so builds need no network.
- Feature `validation-garde` with the `Garde*` extractors.

### Fixed

- A malformed request body no longer leaks the serde error text into
  `message`, and the internal `system` field is attached only for
  `Internal` errors while debug mode is on.
- Debug mode is no longer a mutable global (`OnceLock`), and tests use
  `into_response_with(debug)` instead of flipping process state.

### Documentation

- Module docs cover the response contract, the extractor matrix (source and
  failure status per extractor), and the utoipa handler conventions.
