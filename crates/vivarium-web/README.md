# vivarium-web

HTTP ergonomics on top of [axum](https://github.com/tokio-rs/axum): a unified
`ApiError` response contract, extractors that deserialize + initialize +
validate in one pipeline, JWT authentication, and a `Cache-Control` layer.

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
  (`{"code","message"}`, plus `system` only in debug mode)
- `Varser` / `QueryVarser` / `PathVarser` / `FormVarser` — body/query/path/form
  extractors running deserialize → initialize → validate
- `jwt` — HS256 sign/verify (`sign_token` / `decode_token`, RS256 rejected)
  and `jwt_auth` middleware inserting claims into extensions
- `Cache { seconds }` — tower layer emitting `Cache-Control`
- `serve` — startup helper returning `Result`

MSRV: Rust 1.85.
