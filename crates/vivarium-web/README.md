# vivarium-web

HTTP ergonomics on top of [axum](https://github.com/tokio-rs/axum): a unified
`ApiError` response contract, extractors that deserialize + initialize +
validate in one pipeline, JWT + cookie-session authentication,
refresh-token rotation, RBAC permission wildcards, password hashing, and a
`Cache-Control` layer.

## Quick start

```rust,ignore
use axum::{Router, routing::post};
use garde::Validate;
use serde::Deserialize;
use vivarium_web::{ApiError, Initializer, Varser};

#[derive(Deserialize, Validate)]
struct NewUser {
    #[garde(length(chars, min = 1, max = 100))]
    name: String,
}

impl Initializer for NewUser {} // post-deserialization hook

async fn create_user(Varser(user): Varser<NewUser>) -> Result<(), ApiError> {
    // user.name is trimmed, validated, and ready
    Ok(())
}

let app = Router::new().route("/users", post(create_user));
```

Invalid bodies get `422` with a Go-style message —
`{"code":"VALIDATION","message":"[name]: [length is lower than 1]"}`.
The `code` field is the machine contract; the message is human-readable.

## What's inside

- `ApiError` — one error type, one JSON shape
  (`{"code","message"}`, plus `system` only in debug mode); codes:
  `NOT_FOUND`, `BAD_REQUEST`, `UNAUTHORIZED` (401), `FORBIDDEN` (403),
  `CONFLICT` (409), `VALIDATION`, `INTERNAL`
- `Varser` / `QueryVarser` / `PathVarser` / `FormVarser` — body/query/path/form
  extractors running deserialize → initialize → validate
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
- `Cache { seconds }` — tower layer emitting `Cache-Control`
- `serve` — startup helper returning `Result`

MSRV: Rust 1.85.
