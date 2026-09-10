# vivarium

Type-safe ergonomics on top of [sqlx] and [axum]: pagination, sorting
whitelists, one response envelope, validated extractors, generic CRUD with
typed filters, cookie sessions with sliding renewal, refresh-token rotation,
and hot-reloadable config. A Rust port of the Go [natools4go] toolset — but the
value is not in ported utility functions (Rust's ecosystem covers those), it
is in the type-safe layer built on sqlx/axum.

| Crate | What |
| --- | --- |
| [`vivarium-core`] | `Page<T>` / `Pagination` / `Order` / `Column` / `Sorter` / `Entity` / `PrimaryKey` / `Value` |
| [`vivarium-macros`] | `#[derive(Entity)]` |
| [`vivarium-db`] | sqlx: generic CRUD, `Predicate` filters, `Update`, transactions, pagination |
| [`vivarium-web`] | axum: `ApiResponse` / `ApiError`, extractors, JWT + session auth, refresh tokens, RBAC permissions, password hashing, OpenAPI helpers |
| [`vivarium-config`] | figment + notify + arc-swap hot reload |
| [`vivarium-rs`] | the facade: `cargo add vivarium-rs` is all you need |

MSRV: 1.94. Drivers: SQLite, PostgreSQL, MySQL (sqlx 0.9 has no MSSQL driver,
so there is no `db-mssql` feature). Library strings (including default error
messages) are English; applications localize them through `install_texts`.

## Quick start

```toml
[dependencies]
vivarium-rs = "0.3"       # default features: web, config, db, db-sqlite, db-postgres
anyhow = "1"              # only for `main`'s error type in the example below
axum = "0.8"
sqlx = { version = "0.9", features = ["sqlite", "migrate"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
validator = { version = "0.20", features = ["derive"] }
chrono = "0.4"            # the auth stores take `DateTime<Utc>`
```

### 1. A sqlite + axum service

```rust,no_run
use axum::{Router, routing::post};
use serde::{Deserialize, Serialize};
use validator::Validate;
use vivarium_rs::sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_rs::{ApiError, ApiResponse, Initializer, Varser, create};

#[derive(Clone, sqlx::FromRow, vivarium_rs::Entity)]
#[entity(table = "users")]
struct User {
    id: i64,
    name: String,
}

#[derive(Deserialize, Validate)]
struct NewUser {
    #[validate(length(min = 1, max = 100, message = "name must be 1-100 characters"))]
    name: String,
}

impl Initializer for NewUser {} // run custom field initialization post-parse

#[derive(Serialize)]
struct UserJson {
    id: i64,
    name: String,
}

async fn create_user(
    state: axum::extract::State<SqlitePool>,
    Varser(new_user): Varser<NewUser>,   // deserialize + initialize + validate
) -> Result<ApiResponse<UserJson>, ApiError> {
    let id = create(
        &state.0,
        User {
            id: 0,
            name: new_user.name.clone(),
        },
    )
    .await
    .map_err(ApiError::database)?;
    Ok(ApiResponse::ok(UserJson {
        id,
        name: new_user.name,
    }))
}

# async fn migrations(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
#     // Stands in for the application's own `sqlx::migrate!("./migrations")`,
#     // which needs a migration directory next to the crate that calls it.
#     sqlx::migrate!("../vivarium-db/examples/migrations")
#         .run(pool)
#         .await
# }
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = SqlitePoolOptions::new().connect("sqlite://app.db").await?;
    // Migrations are application-owned: the library ships no migrator, so a
    // library-owned `_sqlx_migrations` table can never collide with yours.
    // Copy `vivarium-db/examples/migrations/` as a starting layout, then:
    //     sqlx::migrate!("./migrations").run(&pool).await?;
    migrations(&pool).await?;
    let app = Router::new()
        .route("/users", post(create_user))
        .with_state(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    vivarium_rs::serve::serve(listener, app).await?;
    Ok(())
}
```

Every handler returns `ApiResponse<T>`; every failure is an `ApiError`. Both
render the same body, so there is exactly one serialization path:

```json
{ "code": 0,   "message": "ok", "data": { "id": 1, "name": "ada" } }
{ "code": 400, "message": "invalid request data", "data": null }
{ "code": 422, "message": "validation failed",
  "errors": { "name": [{ "code": "length", "message": "name must be 1-100 characters",
                         "params": { "min": 1, "max": 100 } }] },
  "data": null }
{ "code": 500, "message": "database error", "data": null }
```

`code` is `0` on success and otherwise the HTTP status code, `data` is always
present, `errors` appears only for a failed validation, and `system` appears
only for an internal error while debug mode is on (`VIVARIUM_DEBUG=1` or
`vivarium_rs::install_debug_mode(true)`).

### 2. Chainable queries

```rust,no_run
use vivarium_rs::{Column, Order, Predicate, Query, Sorter};

# #[derive(Clone, sqlx::FromRow, vivarium_rs::Entity)]
# #[entity(table = "users")]
# struct User {
#     id: i64,
#     name: String,
# }
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
        .filter(Predicate::starts_with(UserCol::Name, "ada"))
        .order_by(Sorter::new(UserCol::Name, Order::Desc))
        .limit(50)
        .find(pool)
        .await
}
```

Column names can only come from a `Column` impl — stringly-typed SQL injection
is impossible, and every value is bound. `Query` also offers `first`
(`ORDER BY … LIMIT 1`), `count`, `paginate`, `paginate_with_total`, `select`
(projection), `filter` with the full `Predicate` tree (`and`/`or`/`negate`,
`one_of`/`not_one_of`, `is_null`/`is_not_null`, `like` — `starts_with`/
`contains` escape `%`/`_` so they match literally), and
`raw_where(RawFragment)` as the explicit escape hatch (`?` is reserved for
binds there).

Partial updates and transactions stay in the same typed layer:

```rust,no_run
use vivarium_rs::{Column, Expr, Update, create, with_transaction};

# #[derive(Clone, sqlx::FromRow, vivarium_rs::Entity)]
# #[entity(table = "users")]
# struct User {
#     id: i64,
#     name: String,
# }
# #[derive(Clone, Copy)]
# enum UserCol { Name, Age }
# impl Column for UserCol {
#     fn name(&self) -> &'static str {
#         match self { UserCol::Name => "name", UserCol::Age => "age" }
#     }
# }
async fn example(pool: &vivarium_rs::sqlx::sqlite::SqlitePool) -> sqlx::Result<()> {
Update::<User, UserCol>::new(1_i64)
    .set(UserCol::Name, "ada")
    .execute(pool)
    .await?;

with_transaction(pool, async |tx| {
    create(&mut **tx, User { id: 0, name: "ada".into() }).await?;
    Update::<User, UserCol>::new(1_i64)
        .set_expr(UserCol::Name, Expr::Now)
        .execute(&mut **tx)
        .await?;
    Ok(())
})
.await?;
Ok(())
}
```

### 3. Pagination

```rust,no_run
use vivarium_rs::Query;

# #[derive(Clone, sqlx::FromRow, vivarium_rs::Entity)]
# #[entity(table = "users")]
# struct User {
#     id: i64,
#     name: String,
# }
# async fn example(pool: vivarium_rs::sqlx::sqlite::SqlitePool) -> sqlx::Result<()> {
let page = Query::<_, User>::new()
    .paginate(vivarium_rs::Pagination::new(2, 20), &pool)
    .await?;
// page.items, page.total, page.page, page.per_page, page.pages()
// out-of-range page/per_page values are normalized
# Ok(())
# }
```

### 4. JWT in three lines

```rust,no_run
use serde::{Deserialize, Serialize};
use vivarium_rs::jwt::{decode_token, sign_token};

# const SECRET: &str = "app-secret";
# fn example() -> Result<(), Box<dyn std::error::Error>> {
#[derive(Serialize, Deserialize)]
struct Claims {
    sub: u64,
    exp: i64,
}

let token = sign_token(&Claims { sub: 7, exp: 0 }, SECRET)?;
let claims: Claims = decode_token(&token, SECRET)?;   // HS256 fixed; RS256 rejected
# assert_eq!(claims.sub, 7);
# Ok(())
# }
```

Or as middleware: `.route_layer(vivarium_rs::jwt::jwt_auth::<Claims>(SECRET.into()))`
puts the decoded claims into request extensions. [`JwtVerifier`] wraps the same
machinery with a configuration (leeway, `aud`/`iss`, required claims) and a
key ring that keeps retired secrets verifying through a rotation.

[`JwtVerifier`]: https://docs.rs/vivarium-web/latest/vivarium_web/jwt/struct.JwtVerifier.html

### 5. Sessions with a sliding TTL and an absolute cap

```rust,no_run
use std::time::Duration;
use axum::{Router, routing::get};
use vivarium_rs::{CookieOptions, SessionAuth, SessionCtx, session_layer};

# use chrono::{DateTime, Utc};
# use vivarium_rs::{ApiError, SessionId, SessionRecord, SessionStore};
# #[derive(Clone, Default)]
# struct MyStore;
# impl SessionStore for MyStore {
#     type UserId = u64;
#     async fn create(&self, _id: &str, _user: u64, _created_at: DateTime<Utc>,
#         _expires_at: DateTime<Utc>, _last_activity: DateTime<Utc>) -> Result<(), ApiError> { Ok(()) }
#     async fn find(&self, _id: &str) -> Result<Option<SessionRecord<u64>>, ApiError> { Ok(None) }
#     async fn touch(&self, _id: &str, _expires_at: DateTime<Utc>, _last_activity: DateTime<Utc>)
#         -> Result<(), ApiError> { Ok(()) }
#     async fn remove(&self, _id: &str) -> Result<bool, ApiError> { Ok(false) }
#     async fn remove_by_user(&self, _user: u64) -> Result<u64, ApiError> { Ok(0) }
# }
# fn example() {
let my_store = MyStore;                          // yours: the store is app-owned
let auth = SessionAuth::new(
    my_store,
    CookieOptions::new("sid"),                   // Secure; HttpOnly; SameSite=Lax; Path=/
    Duration::from_hours(24),                    // slides while the session is used
    Some(Duration::from_hours(24 * 7)),          // absolute cap, never extended past
);
let app: Router = Router::new().route("/me", get(|SessionCtx { user_id }: SessionCtx<u64>| async move {
    user_id.to_string()
}))
    .layer(session_layer(auth));
# let _ = app;
# }
```

The middleware looks up the session by its SHA-256 digest (deleting dead
rows), refreshes the cookie after half the TTL is used, and never rejects —
the `SessionCtx` extractor answers 401 (`OptionalSessionCtx` for
optional-login routes, `SessionId` when the handler needs to log out).
Logins call `auth.start(user_id)` and set the cookie via
`auth.set_cookie_value(&id)`; only the digest reaches the store, so a leaked
session table hands out no live sessions.

Single-use refresh-token rotation and Argon2 password hashing ride along:
`RefreshTokenManager::new(store, secret, access_ttl, refresh_ttl)` — `rotate`
consumes the token, rebuilds the claims from the stored user id, and overwrites
`sub`/`exp`/`iat`, so a replayed or stolen token cannot mint one for someone
else — and `verify_and_upgrade(password, user_hash.as_deref(), Argon2Params::default())`,
which keeps the unknown-user path constant-time against username enumeration
and reports when a stored hash should be upgraded.

### 6. Hot-reloadable config

```rust,no_run
use std::sync::Arc;
use vivarium_rs::{Config, ConfigOptions};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
#[derive(serde::Deserialize, serde::Serialize)]
struct AppConfig {
    port: u16,
}

let config = Arc::new(Config::load_with(
    ConfigOptions::new("config.toml")
        .defaults(&AppConfig { port: 8080 })   // 1. defaults
        .file()                                // 2. config.toml (optional)
        .env_prefixed("APP")                   // 3. APP__* variables
        .separator("__"),
)?);
config.register(|cfg: &AppConfig| println!("port is now {}", cfg.port));
config.on_error(|err| eprintln!("config error: {err}"));
let watcher = Arc::clone(&config).watch()?;   // stops when the watcher drops
let cfg = config.get();                       // lock-free Arc read
# assert_eq!(cfg.port, 8080);
# drop(watcher);
# Ok(())
# }
```

### 7. Validation errors and localization

`Varser` rejects with `400` for malformed input and `422` for semantic
validation failures, with a structured `errors` payload keyed by field:

```json
{ "code": 422, "message": "validation failed",
  "errors": { "username": [{ "code": "length", "message": "用户名长度需在 3-20 之间",
                             "params": { "min": 3, "max": 20 } }] },
  "data": null }
```

The envelope `message` comes from the library's catalog — or straight from the
call site when a handler supplies its own, as in `ApiError::not_found("no such
user")`. The catalog is English until the application installs its own once at
startup —
`install_texts(Texts { unauthorized: "缺少会话 Cookie".into(), ..Texts::default() })`
— and `echo_details` additionally lets a `4xx` message repeat the parser's
detail (useful when a custom `Deserialize` error must reach the client). A
violation's own `message` is whatever the DTO declared in its
`#[validate(message = "…")]` attribute, and the submitted field value is never
echoed back in `params`.

## Features

| Feature | Enables |
| --- | --- |
| `db` | query/CRUD layer, no driver (also turns on `vivarium-web/sqlx`, i.e. `ApiError::conflict_from_db`) |
| `db-sqlite` / `db-postgres` / `db-mysql` | the matching driver (`db-mysql` enables sqlx's `mysql-rsa` for non-TLS servers) |
| `web` | axum layer |
| `config` | hot-reloadable config |
| `validation-garde` | the `Garde*` extractors (bound to `garde::Validate`) in addition to the default validator-based ones |
| `utoipa` / `utoipa-ui` | `ToSchema` derives plus the OpenAPI helpers (`utoipa-ui` downloads nothing: Swagger UI assets are vendored) |
| `telemetry` | `serve::telemetry::init` — stdout lines plus optional daily-rotated JSON log files (requires `web`) |

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
