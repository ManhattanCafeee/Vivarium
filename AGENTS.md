# Repository Guidelines

## Project Overview

Rust workspace providing type-safe ergonomics on top of `sqlx` 0.9 and `axum` 0.8: pagination, compile-time sorting whitelists, one response envelope (`ApiResponse`/`ApiError`), validated extractors, generic CRUD with typed filters, transactions, chainable queries, and hot-reloadable config. A port of the Go `natools4go` toolset — only the type-safe layer, not utility packages. MSRV 1.94, edition 2024, resolver 3, MIT. Library strings (including default messages) are English; applications localize through `vivarium_web::install_texts`.

## Architecture & Data Flow

Six crates, layered bottom-up (publish order = same order):

| Crate | Role | Internal deps |
|---|---|---|
| `vivarium-core` | Shared vocabulary: `Pagination`, `Page<T>`, `Order`, `Column`, `Sorter`, `Value`/`NullType`, `PrimaryKey`, `Entity` trait, `EncodeError` | none (serde + serde_json; optional chrono/uuid/utoipa) |
| `vivarium-macros` | Proc-macro `#[derive(Entity)]` (`#[entity(...)]` attrs) | none (syn/quote) |
| `vivarium-db` | sqlx layer: generic CRUD, `Query<DB, T>` builder + `Predicate` filters, `Update`, `with_transaction`, no migrator | core + macros + sqlx |
| `vivarium-web` | axum layer: `ApiResponse`/`ApiError` envelope, `Texts` catalog, `Varser` family, JWT/session/refresh-token auth (digest-only storage), Argon2 passwords, `CacheControl` layer, OpenAPI helpers | **none — standalone** (optional `sqlx` feature for `conflict_from_db`, optional `telemetry` for `serve::telemetry`) |
| `vivarium-config` | figment + notify + arc-swap hot reload | **none — standalone** |
| `vivarium-rs` | Facade: feature-gated re-exports of everything | all of the above |

Request flow: `Router` → optional `jwt_auth::<T>`/`JwtVerifier::layer` (verifies HS256, inserts claims into request extensions) or `session_layer` (resolves the session cookie, inserts `SessionCtx`/`SessionId`) → optional `CacheControl` layer (stamps `Cache-Control` on 2xx/3xx only, never over an existing header) → handler with a `Varser<T>` extractor → deserialize → `Initializer::try_initialize` → `validator::Validate` → handler calls db helpers with `&pool` as generic `impl Executor` → `Query` assembles `Vec<Step>` (SQL text + `Value` enum binds) → per-driver sealed `DriverOps` builds a sqlx `QueryBuilder` → `T: FromRow` decodes → failures become `ApiError` (a `kind` plus, for 5xx debug, a `system` field) → the single envelope renders `{"code","message","data"}` (plus `errors` on a validation failure).

Config flow: `Config::load` (`ConfigOptions::file_required`) or `Config::load_with(ConfigOptions)` — defaults → file → prefixed env → user providers, later wins — → `get()` returns lock-free `Arc<T>` snapshot (`ArcSwap`) → `watch()` spawns a **std thread** (`notify::RecommendedWatcher` by default, `poll_interval()` opts into `PollWatcher`; parent-dir watch + 100 ms debounce) and returns a `ConfigWatcher` that stops and joins on `stop()`/`Drop` → reload skips no-op values, then fires handlers in registration order; a panicking or failing handler goes to `on_error` and watching continues. Config is intentionally sync/threaded, not tokio.

Key design decisions: primary keys implement `PrimaryKey` (`i64`/`u64`/`i32`/`u32`/`String`; a `u64` above `i64::MAX` errors instead of wrapping); `where_eq` plus the `Predicate` AST cover filtering (joins → native sqlx, documented); binds are the closed `Value` enum (`Into<Value>`, `TypedNull` for typed NULLs, no trait objects); `delete` by id; `update_by_id` sets all non-id columns while `Update` sets a subset; `with_transaction` uses an `AsyncFnOnce` bound so the closure can hold the transaction across `await`; JWT pinned to HS256 (RS256 rejected); MSSQL absent (sqlx has no driver for it).

## Key Directories

- `crates/*/src/` — library source; `crates/vivarium-db/src/driver.rs` is private sealed driver glue (per-driver execution; boxed futures dodge rustc #100013)
- `crates/vivarium-db/examples/migrations/` — sample schema for downstreams and for the crate's own tests (`sqlx::migrate!("./examples/migrations")`); the library embeds **no** migrator
- `crates/vivarium-macros/tests/ui/{pass,fail}/` — trybuild UI cases + `.stderr` snapshots
- `docs/` — **gitignored** planning docs (`go-rust-mapping.md`, `vivarium-plan_副本.md`, Chinese); exists locally, never commit
- `.github/workflows/ci.yml` — the only CI; the canonical dev-command source
- `crates/vivarium-macros/wip/` — self-ignored scratch space

## Development Commands

No Makefile/justfile/xtask/scripts. CI (`.github/workflows/ci.yml`) is authoritative — run all of:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo test --workspace --all-features
cargo doc --no-deps --all-features
SQLX_OFFLINE=true cargo check --workspace --all-features
```

- Postgres tests self-skip without `DATABASE_URL`; to run them: `DATABASE_URL=postgres://postgres:postgres@localhost:5432/vivarium cargo test --workspace --all-features` (CI: postgres:16 service)
- MSRV gate: `cargo +1.94.0 check --workspace --all-features` (CI matrix runs stable + 1.94.0)
- Path dependency versions must equal the workspace crate versions (`vivarium-rs` requires `vivarium-core = "0.1.2"`, `vivarium-db = "0.2.1"`, …); CI checks this with `cargo metadata --no-deps` + `jq`, because a stale requirement only fails at `cargo publish` time
- MySQL tests self-skip without `MYSQL_DATABASE_URL` (CI: mysql:8.4 service); PostgreSQL uses `DATABASE_URL`
- Regenerate trybuild snapshots: `TRYBUILD=overwrite cargo test -p vivarium-macros --test ui`
- `cargo test -p vivarium-db` without features compiles **zero** integration tests (all driver-gated; the sqlx-dependent doctests are `#[cfg]`-gated so the run still succeeds); use `--features sqlite,postgres,mysql`

## Code Conventions & Common Patterns

- **Lint gates (enforced by CI):** `#![deny(missing_docs)]` in all six crates — every public item needs a doc comment. `#![forbid(unsafe_code)]` in core/db/macros only; web's sole `unsafe` is the pin-projection in `cache.rs` (with SAFETY comment)
- **No-panic rule:** no `unwrap()` in library code (`expect` only in tests); `serve()` returns `io::Result` instead of panicking
- **Error handling:** db exposes `pub type Error = sqlx::Error` (pure passthrough; `EncodeError`/`PrimaryKeyError` map to `sqlx::Error::Encode`); web/config use `thiserror` 2. No `anyhow` in src. Web converts at call sites (`.map_err(ApiError::database)`) — no blanket `From` impls beyond the explicit table in `error.rs`. `ApiError` wire contract: `{"code": <i32>, "message", "data"}` with `errors` on a validation failure; the internal `system` field appears **only** for `Internal` in debug mode (`VIVARIUM_DEBUG=1` or `install_debug_mode(true)`); default texts live in `vivarium_web::texts::Texts`
- **Column-name safety:** `Query`/`Sorter` take `impl Column` (enum impls with `fn name(&self) -> &'static str`) — never strings; the stringly-typed path is impossible by construction. Identifiers are quoted per driver (`"` SQLite/PG, backtick MySQL) at splice time, so SQL-keyword names (`order`, `desc`) are safe; `Column` impls carry logical names only
- **`#[derive(Entity)]`:** default anchor crate is `::vivarium_rs` — code outside the facade must add `#[entity(crate = "vivarium_db")]`, and the anchor crate must re-export `Entity`, `Value`, `NullType`, `EncodeError`. Attributes: `table`, `crate`, `id` (must be `i64`/`u64`/`i32`/`u32`/`String`), `rename`, `json`, `skip`; unknown keys are compile errors. `Option<T>` binds as `Value::TypedNull(NullType::…)`, `#[entity(json)]` failures surface as `EncodeError` (never a panic), and `chrono`/`uuid` field types are recognised by their last path segment
- **Async:** db helpers take generic `impl Executor<'q, Database = DB>`; pass `&pool` (`paginate` needs `E: Copy`), and inside `with_transaction`'s `async |tx|` closure pass `&mut **tx`. Web extractors are hand-written `FromRequest`/`FromRequestParts` impls with `Rejection = ApiError`. Driver execution lives in per-driver impls with deliberately boxed futures (`Pin<Box<dyn Future + Send>>`): `impl Future` (RPITIT) leaks the `E: Executor` obligation into handler futures and breaks axum `Send` generalization (rustc #100013) — see `driver.rs` rationale before "optimizing"
- **Sealing/glue:** `mod private { pub trait Sealed {} }` for sealed traits; internal-but-public bounds re-exported `#[doc(hidden)]`
- **Bearer credentials are stored as digests:** session ids and refresh tokens are 32 CSPRNG bytes in base64url (`secrets::generate_token`), and the stores only ever receive `hash_token` (SHA-256 hex, 64 chars) — never the value the client holds. The auth stores are generic over the application's `type UserId` and take `chrono::DateTime<Utc>`; cookie defaults are `Secure; HttpOnly; SameSite=Lax; Path=/` with `CookieOptions::insecure()` as the explicit development opt-out
- **Feature gating:** driver impls behind `#[cfg(feature = "sqlite")]` etc.; facade uses `dep:` optional deps. `vivarium-db` has `default = []` (no driver); `vivarium-core` has `chrono`/`uuid`/`utoipa`; `vivarium-web` has `validation-validator` (default), `validation-garde`, `utoipa`, `utoipa-ui`, `sqlx`, `telemetry`; `vivarium-rs` default = web+config+db+sqlite+postgres (no mysql) and passes `utoipa`/`utoipa-ui`/`validation-garde`/`telemetry` through
- **Docs:** `//!` crate docs; no `# Examples` sections anywhere — examples live in crate-level `no_run` doctests or on key items. Intra-doc links: bare `[`sqlx`]` for direct deps, reference-style docs.rs links for sibling crates. **The root `README.md` is compiled as doctests** by `vivarium-rs` (`#[cfg(doctest)]` + `include_str!`, gated on `web`/`config`/`db-sqlite`), so its `rust` snippets must keep compiling — and because that file is also the crates.io/GitHub front page, where nothing is hidden, the snippets stay self-contained instead of using rustdoc `# ` scaffolding: they declare the types and secrets they touch, or are generic over them
- **Tests:** inline `#[cfg(test)] mod tests` at file bottom; plain `assert!`/`assert_eq!` (no custom assertion crates); handler tests drive `Router::oneshot` via `tower::ServiceExt` (no sockets — except `serve.rs`, which must bind a real `TcpListener` because `axum::serve` cannot be driven through `oneshot`); db tests build per-file `pool()` = `SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:")` + `sqlx::migrate!("./examples/migrations").run` (`max_connections(1)` is mandatory for `sqlite::memory:`); driver-specific test files carry `#![cfg(feature = "sqlite")]`/`mysql` headers and self-skip without their env var

## Important Files

- `Cargo.toml` — workspace manifest (`workspace.package`; note crate versions are NOT inherited and are not uniform: core 0.1.2, macros 0.1.2, db 0.2.1, web 0.2.1, config 0.1.1, rs 0.2.1 — every internal `path` dependency's `version` requirement must equal the crate it points at, which CI checks)
- `crates/vivarium-db/src/lib.rs` — re-export surface (`encode_error`/`key_error` live here)
- `crates/vivarium-db/src/{query,crud,driver}.rs` — query builder, CRUD, driver glue
- `crates/vivarium-db/src/{predicate,update,transaction}.rs` — filter AST, partial updates, transactions
- `crates/vivarium-web/src/{error,response,texts,validation,varser}.rs` — envelope + error kinds, message catalog, structured validation, extractor pipeline
- `crates/vivarium-web/src/{jwt,session,token,password,secrets,cache,serve}.rs` — auth (`JwtVerifier` + key ring, digest-keyed session/refresh stores, Argon2 with parameter upgrades), credential hashing, `CacheControl`, serving + optional `telemetry`
- `crates/vivarium-web/src/openapi.rs` — utoipa security schemes, `info`, `mount` (feature `utoipa`/`utoipa-ui`)
- `crates/vivarium-config/src/lib.rs` — single-module crate; reload/watch semantics
- `crates/vivarium-rs/src/lib.rs` — facade re-export matrix + feature definitions
- `.github/workflows/ci.yml` — full gate (fmt, clippy `-D warnings`, test, single-feature checks, doc, `SQLX_OFFLINE=true` check; matrix stable + 1.94.0; postgres:16 + mysql:8.4 services)
- `README.md` — quick start; `docs/go-rust-mapping.md` — Go→Rust porting map (non-ports rationale)
- `crates/vivarium-db/examples/migrations/0001_create_users.sql` (+ `.down.sql`) — sample migration layout, not compiled into the library

## Runtime/Tooling Preferences

- **Runtime/toolchain:** stable Rust with **MSRV 1.94** (sqlx 0.9 requires it). No `rust-toolchain.toml` — CI matrix pin `1.94.0` is the enforcement. Trybuild `.stderr` snapshots hold only the derive's own diagnostics, so they are stable across toolchains
- **Style:** default rustfmt (no `rustfmt.toml`), default clippy (no `clippy.toml`) at `-D warnings`
- **Key deps (pinned in Cargo.lock):** sqlx 0.9.0 (no MSSQL; `mysql-rsa` for non-TLS MySQL), axum 0.8.9, validator 0.20 (default backend) + garde 0.22 (optional), utoipa 5.5 + utoipa-axum 0.2 + utoipa-scalar 0.3 + utoipa-swagger-ui 9 (`vendored`, no build-time download), chrono 0.4 + uuid 1, jsonwebtoken 10 (`rust_crypto` — pure Rust, no OpenSSL), thiserror 2, tokio 1.53, tower 0.5, figment 0.10 (with `env`), notify 8, arc-swap 1, tracing
- **sqlx offline mode:** CI runs `SQLX_OFFLINE=true cargo check`. No `query!`-family macros exist yet (no `.sqlx/` cache committed). Adding compile-time-checked queries requires `cargo sqlx prepare` + committing `.sqlx/`, or the offline step breaks
- No docker-compose, no `.env*`, no publish/release workflow, no dependabot

## Testing & QA

- **Command:** `cargo test --workspace --all-features` (works without database servers — `pg.rs`/`mysql.rs` self-skip via `DATABASE_URL`/`MYSQL_DATABASE_URL` + eprintln + return). Doctests run automatically; crate-level examples are `no_run` (compile-only)
- **Layout:** integration tests in `crates/*/tests/` (db, macros, rs — including `vivarium-rs/tests/paths.rs`, which pins the facade's re-export surface by compiling it — plus web's `openapi.rs`/`texts.rs`) + inline `#[cfg(test)]` modules (every web module, config, core, macros). No shared test-utils module — self-contained helpers per file (`pool()`, `drive()`, `body_json()`); duplication is accepted
- **Style to copy:** web handler test = build `Router` + `app.oneshot(req)` (`crates/vivarium-web/src/cache.rs` is the shortest); db test = `#[derive(sqlx::FromRow, Entity)]` + in-memory pool + `sqlx::migrate!("./examples/migrations")` (`crates/vivarium-db/tests/crud.rs`); end-to-end acceptance template = `crates/vivarium-rs/tests/quickstart.rs`; macro behavior = `crates/vivarium-macros/tests/derive.rs`; config reload = `tempfile` + `#[tokio::test(flavor = "multi_thread")]`
- **Coverage:** none — no tarpaulin/llvm-cov/codecov config. The gate is CI: fmt + clippy `-D warnings` + full test suite on stable and 1.94.0
