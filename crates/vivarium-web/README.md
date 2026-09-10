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
- `jwt` — HS256 sign/verify (`sign_token` / `decode_token`, RS256 rejected)
  and `jwt_auth` middleware inserting claims into extensions (failures → 401)
- `session` — cookie sessions with sliding renewal: `SessionAuth`,
  `SessionStore` (app-owned table), `session_layer`, `SessionCtx` /
  `OptionalSessionCtx`; expired sessions are deleted, stale cookies cleared
- `token` — single-use refresh-token rotation: `RefreshTokenManager`,
  `RefreshTokenStore`, `TokenPair`; a replayed or raced token fails 401
- `authz` — RBAC permission wildcards: `perms_match` (`*`, exact,
  `prefix:*`) and `PermissionSet` with `require` → 403
- `password` — Argon2 `hash` / `verify` / `verify_login`; the unknown-user
  login path verifies a dummy hash, defeating username-enumeration timing
- `cache::Cache { seconds }` — tower layer emitting `Cache-Control`
- `serve` — startup helper returning `Result`
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

MSRV: Rust 1.94.
