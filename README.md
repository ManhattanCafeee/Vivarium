# vivarium

Type-safe ergonomics on top of [sqlx] and [axum]: pagination, sorting
whitelists, unified errors, validated extractors, generic CRUD, cookie
sessions with sliding renewal, refresh-token rotation, and hot-reloadable
config. A Rust port of the Go [natools4go] toolset — but the
value is not in ported utility functions (Rust's ecosystem covers those), it
is in the type-safe layer built on sqlx/axum.

| Crate | What |
| --- | --- |
| [`vivarium-core`] | `Page<T>` / `Pagination` / `Order` / `Column` / `Sorter` / `Entity` / `Value` |
| [`vivarium-macros`] | `#[derive(Entity)]` |
| [`vivarium-db`] | sqlx: generic CRUD, chainable queries, pagination, migrations |
| [`vivarium-web`] | axum: `ApiError`, `Varser`, JWT + session auth, refresh tokens, RBAC permissions, password hashing, `Cache-Control` layer |
| [`vivarium-config`] | figment + notify + arc-swap hot reload |
| [`vivarium-rs`] | the facade: `cargo add vivarium-rs` is all you need |

MSRV: 1.85. Drivers: SQLite, PostgreSQL, MySQL (sqlx 0.8 removed the MSSQL
driver, so there is no `db-mssql` feature).

## Quick start

```toml
[dependencies]
vivarium-rs = "0.1"       # default features: web, config, db, db-sqlite, db-postgres
axum = "0.8"
sqlx = { version = "0.8", features = ["sqlite"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
garde = { version = "0.22", features = ["derive"] }
```

### 1. A sqlite + axum service

```rust,no_run
use axum::{Router, routing::post};
use serde::Deserialize;
use garde::Validate;
use vivarium_rs::{ApiError, Initializer, Varser, create};
use vivarium_rs::sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

#[derive(Clone, sqlx::FromRow, vivarium_rs::Entity)]
#[entity(table = "users")]
struct User {
    id: i64,
    name: String,
}

#[derive(Deserialize, Validate)]
struct NewUser {
    #[garde(length(chars, min = 1, max = 100))]
    name: String,
}

impl Initializer for NewUser {} // run custom field initialization post-parse

async fn create_user(
    state: axum::extract::State<SqlitePool>,
    Varser(new_user): Varser<NewUser>,   // deserialize + initialize + validate
) -> Result<(), ApiError> {
    create(&state.0, User { id: 0, name: new_user.name })
        .await
        .map_err(|e| ApiError::Internal { system: e.to_string() })?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = SqlitePoolOptions::new().connect("sqlite://app.db").await?;
    vivarium_rs::MIGRATOR.run(&pool).await?;
    let app = Router::new()
        .route("/users", post(create_user))
        .with_state(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    vivarium_rs::serve(listener, app).await?;
    Ok(())
}
```

Invalid bodies get `422 {"code":"VALIDATION","message":"[name]: [length is lower than 1]"}`.

### 2. Chainable queries

```rust,no_run
use vivarium_rs::{Column, Order, Query, Sorter};

#[derive(Clone, Copy)]
enum UserCol { Name, Age }
impl Column for UserCol {
    fn name(&self) -> &'static str {
        match self { UserCol::Name => "name", UserCol::Age => "age" }
    }
}

async fn active_above(pool: &vivarium_rs::sqlx::sqlite::SqlitePool, age: i64) -> sqlx::Result<Vec<User>> {
    Query::<_, User>::new()
        .where_eq(UserCol::Age, age)          // closed Value bind: compile-time checked
        .order_by(Sorter::new(UserCol::Name, Order::Desc))
        .limit(50)
        .find(pool)
        .await
}
```

Column names can only come from a `Column` impl — stringly-typed SQL
injection is impossible. `Query` also has `first`, `count`, and `paginate`.

### 3. Pagination

```rust,no_run
let page = Query::<_, User>::new()
    .paginate(vivarium_rs::Pagination::new(2, 20), &pool)
    .await?;
// page.total, page.content, page.pages(); out-of-range sizes normalized
```

### 4. JWT in three lines

```rust,no_run
use vivarium_rs::jwt::{decode_token, sign_token};

let token = sign_token(&Claims { sub: "ada".into(), exp: 0 }, SECRET)?;
let claims: Claims = decode_token(&token, SECRET)?;   // HS256 fixed; RS256 rejected
```

Or as middleware: `.route_layer(vivarium_rs::jwt::jwt_auth::<Claims>(SECRET.into()))`
puts the decoded claims into request extensions.

### 5. Sessions with sliding renewal

```rust,no_run
use std::time::Duration;
use axum::{Router, routing::get};
use vivarium_rs::{SessionAuth, SessionCtx, session_layer};

let auth = SessionAuth::new(my_store, "sid", Duration::from_hours(24)); // SessionStore is app-owned
let app = Router::new().route("/me", get(|SessionCtx { user_id }: SessionCtx| user_id.to_string()))
    .layer(session_layer(auth));
```

The middleware looks up the session (deleting expired ones), refreshes the
cookie after half the TTL is used, and never rejects — the `SessionCtx`
extractor answers 401 (`OptionalSessionCtx` for optional-login routes).
Logins call `auth.start(user_id)` and set the cookie via
`auth.set_cookie_value(id)`.

Single-use refresh-token rotation and Argon2 password hashing ride along:
`RefreshTokenManager::new(store, secret, access_ttl, refresh_ttl)`
(`rotate` consumes the token; a replayed one fails the delete 401) and
`verify_login(password, user_hash.as_deref())`, which keeps the
unknown-user path constant-time against username enumeration.

### 6. Hot-reloadable config

```rust,no_run
use std::sync::Arc;
use vivarium_rs::Config;

#[derive(serde::Deserialize)]
struct AppConfig {
    port: u16,
}

let config = Arc::new(Config::load("config.toml")?);
config.register(|cfg: &AppConfig| println!("port is now {}", cfg.port));
config.clone().watch()?;          // watches the file, re-loads, fires handlers
let cfg = config.get();           // lock-free Arc read
```

### 7. Validation errors

`Varser` rejects with `422` and a Go-style message — `[field]: [rule]`:

```rust
HTTP/1.1 422 Unprocessable Entity
{"code":"VALIDATION","message":"[name]: [length is lower than 1]"}
```

Internal details stay server-side: `ApiError::Internal { system }` only
appears in the response while debug mode is on (`VIVARIUM_DEBUG=1` or
`vivarium_rs::set_debug_mode(true)`).

## Features

| Feature | Enables |
| --- | --- |
| `db` | query/CRUD layer, no driver |
| `db-sqlite` / `db-postgres` / `db-mysql` | the matching driver |
| `web` | axum layer |
| `config` | hot-reloadable config |

Defaults: `web`, `config`, `db`, `db-sqlite`, `db-postgres`.

## Deliberately not ported

The Go toolbox's utility packages (`slice`, `strs`, `jsons`, `rands`, `pwd`,
`concur`, `ask`, `flags`, `dbg`) have direct Rust equivalents and are not
included. The `E/I/Must` triple-state error convention, panic-recovery pools,
viper's untyped getters, GORM-style runtime schema reflection, and DDL
reshuffling are also intentionally absent.

## Release order

`vivarium-core` → `vivarium-macros` → `vivarium-db` → `vivarium-web` →
`vivarium-config` → `vivarium-rs`.

## License

MIT.

[sqlx]: https://github.com/launchbadge/sqlx
[axum]: https://github.com/tokio-rs/axum
[natools4go]: https://github.com/natholdallas/natools4go
[`vivarium-core`]: https://docs.rs/vivarium-core
[`vivarium-macros`]: https://docs.rs/vivarium-macros
[`vivarium-db`]: https://docs.rs/vivarium-db
[`vivarium-web`]: https://docs.rs/vivarium-web
[`vivarium-config`]: https://docs.rs/vivarium-config
[`vivarium-rs`]: https://docs.rs/vivarium-rs
