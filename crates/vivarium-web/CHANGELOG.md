# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Breaking

- Credentials and lifetimes are parsed once, at construction: `KeyRing::new`,
  `sign_token`, `decode_token` and `jwt_auth` take a `Secret`
  (`Secret::try_new` rejects empty and whitespace-only values), and
  `RefreshTokenManager::new` / `SessionAuth::new` take `Ttl` values
  (`Ttl::try_new` rejects `Duration::ZERO`). An empty HS256 key used to sign
  **and** verify, so a deployment whose secret came out empty accepted
  attacker-minted tokens; a zero access/refresh TTL minted tokens that were
  born expired, and a zero session TTL made login silently useless.
- `KeyRing`'s `primary`/`previous` fields are `Secret` now, and its `Debug`
  prints `<redacted>`: the derived one leaked the signing key, unlike
  `SessionId`/`RefreshTokenManager`/`VerifyOutcome`, which already redact.
- `JwtConfig::required_spec_claims` no longer replaces the `exp` requirement
  — `exp` is always required, which is what the field's docs already promised
  ("on top of `exp`"). An empty list, or a mis-cased `"Exp"` entry, no longer
  disables it.
- `SessionId`'s field is private; build it with `SessionId::try_new` (empty
  and whitespace-only ids are rejected). `SessionId::digest()` returns the new
  `Digest` type, and `SessionStore`/`RefreshTokenStore` take `&Digest` rather
  than `&str`, so a store implementation can no longer confuse the raw
  credential with its digest. A store keyed by a `VARCHAR(64)` column keeps
  working through `digest.as_str()`; a store that wants the type itself can
  key by it.
- `Argon2Params`' fields are private: build with
  `Argon2Params::try_new(m_cost, t_cost, p_cost)` (or `Default`) and read them
  through the `m_cost()`/`t_cost()`/`p_cost()` accessors. Zeroed parameters
  used to surface as a 500 on the first `hash()`/login and made `needs_rehash`
  answer "no upgrade" forever.
- `Texts`' fields, `ApiResponse::message` and `FieldViolation::message` are
  `Message` values now: a non-blank client-facing string built with
  `Message::try_new` (or `TryFrom`); there is deliberately no infallible
  `From`. An empty or whitespace-only message used to render as
  `"message": ""` — a blank 404/500 body from an application catalog or a
  DTO's `#[validate(message = "")]`.
- `ApiResponse::error`/`error_with_errors` take a `http::StatusCode` and a
  `Message`, so `ApiResponse::error(0, "")` is no longer expressible; the
  body's `code` still carries the status as an `i32` and the response still
  answers HTTP 200.
- `ValidationErrors`' map is private — build it with `insert`. A blank field
  key (a `garde` struct-level rule has no field path) is stored under `"_"`,
  and the type can no longer hold `{"email": []}`.
- `CookieOptions`' fields are private: build one with
  `CookieOptions::try_new(name)` (empty and whitespace-only names are
  rejected) and refine it with `with_path`/`with_domain`/`with_same_site`/
  `with_secure`/`with_http_only`; every value is checked at construction, so a
  blank `Name=`, `Path=`, or `Domain=` can no longer be emitted. A blank name
  used to make the middleware miss the cookie on every request — all requests
  anonymous while `start` kept writing unclaimable sessions — and made
  `sets_cookie` misread the handler's response.
- `openapi::session_cookie_scheme` takes the `CookieOptions` the session
  middleware uses (`&CookieOptions`) instead of a free `&str` cookie name, so
  the scheme and the middleware cannot name different cookies; and
  `openapi::info` returns `Result<Info, InfoError>` — a blank title or version
  is rejected (OpenAPI requires both to be non-empty) and a blank description
  is omitted instead of being sent as `""`.

### Added

- `CookieOptionsError` (re-exported at the crate root) and
  `openapi::InfoError` — the checked-construction errors of the two builders
  above.
- `secrets::{Secret, Ttl, Digest}`, with `SecretError`/`TtlError`/
  `DigestError`, plus `SessionIdError` and `Argon2ParamsError` — the checked
  values the authentication surface consumes.
- `message::{Message, MessageError}`, re-exported at the crate root, with the
  catalog's field type `Message` also re-exported from `texts` (every catalog
  field is one).

### Fixed

- `ApiError`'s constructors drop a blank explicit message, so
  `ApiError::new(kind, "")` answers with the catalog text for `kind` instead of
  rendering an empty `message`.
- `ValidationErrors` rejects the entry shapes its public map used to allow —
  a blank key or an empty violation list — on deserialization.
- `serve::telemetry` treats a blank filter directive — `TelemetryOptions`'
  `level: Some("")` or an empty `RUST_LOG` — as absent and falls back to
  `info`; it used to install an empty `EnvFilter` that silently logged
  nothing. A non-blank directive that does not parse (including `RUST_LOG`) is
  a `TelemetryError::Filter` instead of silently falling back to `info`.

## 0.3.4 — 2026-09-14

### Changed

- Version bump only, released in lockstep with the 0.3.4 workspace; no code
  changes.

## 0.3.2 — 2026-09-11

### Fixed

- The generated `ApiResponse_*` OpenAPI components describe the envelope with
  the one-line summary again: `ToSchema` derives a schema's `description` from
  the type's whole doc comment, so the rustdoc example above `ApiResponse` was
  folded into every component and generated SDK docs showed an escaped Rust
  snippet.

### Changed

- Docs: `ApiResponse`'s example moved to `ApiResponse::ok`, the constructor it
  exercises, so the struct documentation is the description alone; a new test
  fails if a doc-comment code block is ever folded back into a schema
  description.

## 0.3.1 — 2026-09-10

### Fixed

- `PathVarser`/`QueryVarser` and the garde `GardePathVarser`/`GardeQueryVarser`
  now implement `FromRequestParts`, so handlers that combine a path or query
  extractor with a body extractor compile — `(PathVarser<P>, Varser<B>)` used to
  fail with `E0277`. `Varser`/`FormVarser` and their garde twins still consume
  the body and remain limited to the last argument.

### Changed

- Docs: the session renewal comment no longer claims the renewal can only move
  forward (the clock is re-read after the handler), and the extractor module
  documents which extractors read the body.

### Added

- Tests only: a dev-dependency on `vivarium-core` (with `utoipa`) so the golden
  spec pins the composed `ApiResponse_Page_User` name.

## 0.3.0 — 2026-09-10

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
- Sessions and refresh tokens are **stored as SHA-256 digests**
  (`hash_token`), and both are minted from 32 CSPRNG bytes as base64url
  instead of UUIDv4: the stores never receive the value the client holds, and
  rows from 0.2.x no longer match — clear (or migrate) `sessions` and
  `refresh_tokens`. See the README's upgrade notes for the DDL.
- `SessionStore` / `RefreshTokenStore` take the application's user id type
  (`type UserId`), UTC `chrono::DateTime<Utc>` timestamps, and digest keys;
  `SessionRecord` gained `created_at`, and `remove_by_user` returns the number
  of rows deleted.
- `SessionAuth::new(store, CookieOptions, ttl, absolute_ttl)` replaces
  `(store, name, ttl)`; `SessionAuth::start` returns a `SessionId` (the raw
  value, not a `String`), `end` is idempotent, and cookies default to
  `Secure; HttpOnly; SameSite=Lax; Path=/` (`CookieOptions::insecure` opts out
  for plain-HTTP development). New `SessionId` extractor for logout.
- `RefreshTokenManager::rotate(received, rebuild)` replaces
  `rotate(received, claims)`: the callback receives only the user id read from
  the store, and the manager overwrites `sub`/`exp`/`iat` on the claims it
  returns. `issue` takes `&mut C` and stamps the same fields. `TokenPair`
  gained `expires_in`, and `AccessClaims` gained `set_subject`.
- `jwt`: `sign_token`/`decode_token`/`jwt_auth` keep their signatures but are
  now shorthands over `JwtVerifier`; `Cache` is gone in favour of
  `CacheControl` (see below). A signing failure is now a `500` internal with
  the detail only in the error source — it used to be a `400` whose message
  repeated the serializer error.
- `JwtConfig` also enforces `nbf` (jsonwebtoken leaves it off by default), and
  a configured `audience`/`issuer` now *requires* the claim: a token that omits
  it is rejected instead of skipping the check.
- `SessionAuth::expires_at()` and `cookie_name()` are gone
  (`cookie()` returns the whole `CookieOptions`); `set_cookie_value` takes a
  `&SessionId`; `end` returns `()` and `end_for_user` the number of deleted
  rows, as do `RefreshTokenStore::remove` (`bool`) and `remove_by_user`/`revoke_all` (`u64`).
- `cache::Cache { seconds }` → `CacheControl::{Public, Private, NoCache,
  NoStore}.layer()`; the layer no longer overwrites an existing
  `Cache-Control` and never stamps a 4xx/5xx response (a 401 used to be
  marked `public`).
- `PermissionSet::require` answers the catalog's `forbidden` message without
  appending the refused code (it is logged instead).
- `Texts` gained `invalid_refresh` (the message of a rejected refresh token);
  code that builds the struct literally must add the field, everything else
  keeps working through `..Texts::default()`.
- A `FieldViolation` omits the `message` key when the failed rule declared no
  message; the old implementation serialized `validator` directly and emitted
  `"message": null` instead.
- A `FieldViolation`'s `params` no longer echo the submitted value
  (`params.value`), which may hold a password or a token; the key is omitted.

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
- `secrets::hash_token` — the SHA-256 digest the stores are keyed by, plus the
  32-byte base64url generator behind session ids and refresh tokens.
- `session::CookieOptions` (name/path/secure/http_only/same_site/domain) and
  `SessionAuth::absolute_ttl`, which caps a session's total lifetime next to
  its sliding TTL; renewal shrinks the cookie's `Max-Age` to match.
- `token::Claims<U>` / `AccessClaims<U>` — ready-made access claims (`sub`,
  `exp`, `iat`, `jti`, `scope`, flattened extras) that a manager can stamp.
- `jwt::{JwtConfig, KeyRing, JwtVerifier}` — configurable validation
  (leeway, audience, issuer, required claims), key rotation through retired
  secrets, and `optional_layer` for routes that authenticate when a token is
  present.
- `password::{Argon2Params, hash_with, needs_rehash, verify_and_upgrade,
  VerifyOutcome}` — parameter upgrades on login; `VerifyOutcome`'s `Debug`
  redacts the new hash.
- Feature `telemetry` with `serve::telemetry`: `TelemetryOptions`,
  `TelemetryError` and `TelemetryGuard` (dropping it flushes the file).
- `serve::serve_with_shutdown` and `serve::shutdown_signal` (Ctrl-C plus
  SIGTERM on Unix).

### Fixed

- A malformed request body no longer leaks the serde error text into
  `message`, and the internal `system` field is attached only for
  `Internal` errors while debug mode is on.
- Debug mode is no longer a mutable global (`OnceLock`), and tests use
  `into_response_with(debug)` instead of flipping process state.

### Documentation

- Module docs cover the response contract, the extractor matrix (source and
  failure status per extractor), and the utoipa handler conventions.
