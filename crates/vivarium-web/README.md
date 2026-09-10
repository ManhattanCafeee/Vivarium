# vivarium-web

HTTP ergonomics on top of [axum](https://github.com/tokio-rs/axum): one
response envelope (`ApiResponse` / `ApiError`), extractors that deserialize +
initialize + validate in one step, JWT + cookie-session authentication,
refresh-token rotation, RBAC permission wildcards, password hashing, a
`Cache-Control` layer, and OpenAPI helpers.

## Quick start

```rust,ignore
use axum::{Router, routing::post};
use serde::Deserialize;
use validator::Validate;
use vivarium_web::{ApiError, ApiResponse, Initializer, Varser};

#[derive(Deserialize, Validate)]
struct NewUser {
    #[validate(length(min = 1, max = 100, message = "name must be 1-100 characters"))]
    name: String,
}

impl Initializer for NewUser {} // post-deserialization hook

async fn create_user(Varser(user): Varser<NewUser>) -> Result<ApiResponse<String>, ApiError> {
    // user.name is trimmed, validated, and ready
    Ok(ApiResponse::ok(user.name))
}

let app = Router::new().route("/users", post(create_user));
```

Every response — success or failure — is the same envelope:

```jsonc
{ "code": 0,   "message": "ok", "data": { "id": 1 } }        // 200
{ "code": 400, "message": "invalid request data", "data": null }
{ "code": 404, "message": "not found", "data": null }
{ "code": 422, "message": "validation failed",
  "errors": { "name": [{ "code": "length", "message": "name must be 1-100 characters",
                         "params": { "min": 1, "max": 100 } }] },
  "data": null }
```

`code` is `0` on success and otherwise the HTTP status code; `data` is always
present; `errors` appears only for a failed validation; `system` appears only
for an internal error while debug mode is on. The envelope `message` comes
from `Texts` (English by default), which an application installs once at
startup; a violation's own `message` is the text its DTO declared in
`#[validate(message = "…")]`, and the submitted field value is never echoed
back in `params`.

## What's inside

- `response::ApiResponse<T>` — the envelope; `ApiResponse::ok` / `error` /
  `error_with_errors`, always HTTP 200 (use `(StatusCode, ApiResponse<T>)` when
  you need another status)
- `error::ApiError` + `ErrorKind` — one error type, nine kinds
  (`BadRequest`, `DataParse`, `Validation`, `Unauthorized`, `Forbidden`,
  `NotFound`, `Conflict`, `TooManyRequests`, `Internal`), a per-kind
  constructor, `bail!`, and a `Result<T, E = ApiError>` alias
- `texts::{Texts, install_texts}` — the message catalog, installable once per
  process; `Texts::echo_details` decides whether a 4xx parse failure repeats
  the upstream detail
- `Varser` / `QueryVarser` / `PathVarser` / `FormVarser` — body/query/path/form
  extractors running deserialize → initialize → validate; `validation-validator`
  (default) binds `validator::Validate`, `validation-garde` adds the `Garde*`
  equivalents bound to `garde::Validate`
- `validation::{ValidationErrors, FieldViolation}` — the structured `errors`
  payload, with flattened paths (`profile.email`, `items[0].name`)
- `jwt` — HS256 signing and verification: `JwtVerifier` (`JwtConfig` for
  leeway/audience/issuer/required claims, `KeyRing` holding retired secrets
  through a rotation), `layer` / `optional_layer` middlewares inserting the
  claims into extensions, plus the `sign_token` / `decode_token` / `jwt_auth`
  shorthands (RS256 rejected)
- `session` — cookie sessions with a sliding TTL and an absolute cap:
  `SessionAuth` (`CookieOptions` defaults to `Secure; HttpOnly; SameSite=Lax`),
  `SessionStore` (app-owned table, keyed by the SHA-256 digest of the id),
  `session_layer`, and the `SessionCtx` / `OptionalSessionCtx` / `SessionId`
  extractors; dead sessions are deleted and stale cookies cleared
- `token` — single-use refresh-token rotation: `RefreshTokenManager`,
  `RefreshTokenStore` (digests only), `AccessClaims` / `Claims`, and a
  `TokenPair` carrying `expires_in`; the manager overwrites `sub`/`exp`/`iat`,
  so a replayed, raced or stolen token cannot mint a token for another user
  (401)
- `authz` — RBAC permission wildcards: `perms_match` (`*`, exact,
  `prefix:*`) and `PermissionSet` with `require` → 403
- `password` — Argon2id `hash` / `hash_with` / `verify` / `verify_login` /
  `needs_rehash` / `verify_and_upgrade`; `Argon2Params` defaults to the
  reference profile, so existing hashes keep verifying, and the unknown-user
  login path verifies a dummy hash, defeating username-enumeration timing
- `cache::CacheControl` — `Public(secs)` / `Private(secs)` / `NoCache` /
  `NoStore`, applied with `.layer()`; an existing `Cache-Control` wins and
  4xx/5xx responses are never stamped
- `serve` — `serve`, `serve_with_shutdown` and `shutdown_signal` (Ctrl-C and
  SIGTERM), plus with the `telemetry` feature `telemetry::init`: stdout lines
  and daily-rotated JSON logs, flushed when the returned guard drops
- `openapi` (feature `utoipa`) — `session_cookie_scheme`, `bearer_scheme`,
  `info`, and `mount` (feature `utoipa-ui`: Scalar + Swagger UI +
  `openapi.json`), plus the handler conventions the generated SDKs depend on

## Status codes

| Failure | Status | Kind |
|---|---|---|
| body could not be read / malformed request | 400 | `BadRequest` |
| body is not valid JSON or does not match the type | 400 | `DataParse` |
| a rule failed (details in `errors`) | 422 | `Validation` |
| not authenticated | 401 | `Unauthorized` |
| authenticated but not allowed | 403 | `Forbidden` |
| missing / conflict | 404 / 409 | `NotFound` / `Conflict` |
| rate limited | 429 | `TooManyRequests` |
| unexpected server failure | 500 | `Internal` |

## Features

| Feature | Default | Adds |
|---|---|---|
| `validation-validator` | yes | the canonical `*Varser` extractors |
| `validation-garde` | no | the `Garde*` extractors |
| `utoipa` | no | `ToSchema` derives, security schemes, `info` |
| `utoipa-ui` | no | `openapi::mount` |
| `sqlx` | no | `From<sqlx::Error> for ApiError`, `ApiError::conflict_from_db` |
| `telemetry` | no | `serve::telemetry::init` (stdout + JSON log files) |

MSRV: Rust 1.94.

## Upgrading to 0.3 (auth)

The authentication modules changed shape; two migrations are unavoidable on
the application side.

**Credentials are stored as digests.** Session ids and refresh tokens are
random 32-byte base64url strings now, and the stores only ever receive their
SHA-256 digest (`hash_token`, 64 characters). Existing rows hold raw ids that
no longer match anything, so they must be cleared (or migrated) while everyone
is logged out; the digest column also has to be wide enough:

```sql
-- MySQL dialect; `VARCHAR(64)` replaces the UUID `VARCHAR(36)`.
ALTER TABLE sessions
    MODIFY session_id VARCHAR(64) NOT NULL,   -- sha256 digest
    ADD COLUMN last_activity DATETIME NOT NULL DEFAULT (UTC_TIMESTAMP());
ALTER TABLE refresh_tokens
    MODIFY token VARCHAR(64) NOT NULL,        -- sha256 digest
    ADD COLUMN last_activity DATETIME NOT NULL DEFAULT (UTC_TIMESTAMP());

-- Raw ids are unreadable by 0.3: logging everyone out is the migration.
DELETE FROM sessions;
DELETE FROM refresh_tokens;
```

`SessionRecord` carries `created_at` (the base of `absolute_ttl`) besides
`expires_at` and `last_activity`, and the stores are keyed by the
application's own user id type (`type UserId`), not `i64`.

**Timestamps are `chrono::DateTime<Utc>`.** `SessionStore` and
`RefreshTokenStore` take UTC datetimes instead of `std::time::SystemTime`, so
a `DATETIME`/`TIMESTAMP` column maps directly; epoch seconds are the portable
fallback.

**Cookie and cache behaviour defaults changed.** Sessions default to
`Secure; HttpOnly; SameSite=Lax; Path=/` (call `CookieOptions::insecure()` for
plain-HTTP development), and the cache layer no longer stamps 4xx/5xx
responses — a 401 that used to be marked `public` is now left alone.
