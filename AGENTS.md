# Repository Guidelines

## Project Overview

Rust workspace providing type-safe ergonomics on top of `sqlx` 0.8 and `axum` 0.8: pagination, compile-time sorting whitelists, a unified error type, validated extractors, generic CRUD, chainable queries, and hot-reloadable config. A port of the Go `natools4go` toolset — only the type-safe layer, not utility packages. MSRV 1.85, edition 2024, resolver 3, MIT.

## Architecture & Data Flow

Six crates, layered bottom-up (publish order = same order):

| Crate | Role | Internal deps |
|---|---|---|
| `vivarium-core` | Shared vocabulary: `Pagination`, `Page<T>`, `Order`, `Column`, `Sorter`, `Value`, `Entity` trait | none (serde only) |
| `vivarium-macros` | Proc-macro `#[derive(Entity)]` (`#[entity(...)]` attrs) | none (syn/quote) |
| `vivarium-db` | sqlx layer: generic CRUD, `Query<DB, T>` builder, `MIGRATOR` | core + macros + sqlx |
| `vivarium-web` | axum layer: `ApiError`, `Varser` family, JWT, `Cache` layer | **none — standalone** |
| `vivarium-config` | figment + notify + arc-swap hot reload | **none — standalone** |
| `vivarium-rs` | Facade: feature-gated re-exports of everything | all of the above |

Request flow: `Router` → optional `jwt_auth::<T>` layer (verifies HS256, inserts claims into request extensions) → optional `Cache` layer (stamps `Cache-Control`) → handler with a `Varser<T>` extractor → deserialize → `Initializer::try_initialize` → `garde::Validate` → handler calls db helpers with `&pool` as generic `impl Executor` → `Query` assembles `Vec<Step>` (SQL text + `Value` enum binds) → per-driver sealed `DriverOps` builds a sqlx `QueryBuilder` → `T: FromRow` decodes → errors map to `ApiError::Internal { system }` → `IntoResponse` renders `{"code","message"}`.

Config flow: `Config::load` (by file extension) → `get()` returns lock-free `Arc<T>` snapshot (`ArcSwap`) → `watch()` spawns a **std thread** (`notify::PollWatcher`, 100 ms poll on parent dir, 100 ms debounce) → reload stores new `Arc` then fires handlers in registration order; failed reload keeps the old config. Config is intentionally sync/threaded, not tokio.

Key design decisions (v1 scope): single `i64` primary key only; `where_eq` is the only query operator (joins/Not/Or → `sqlx::query_as!`); binds are the closed `Value` enum (`Into<Value>`, no trait objects); `delete` by id; `update_by_id` sets all non-id columns; JWT pinned to HS256 (RS256 rejected); MSSQL absent (sqlx 0.8 dropped it).

## Key Directories

- `crates/*/src/` — library source; `crates/vivarium-db/src/driver.rs` is private sealed driver glue (per-driver execution; boxed futures dodge rustc #100013)
- `crates/vivarium-db/migrations/` — embedded via `sqlx::migrate!("./migrations")`; **compile-time input**, keep up/down pairs
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
- MSRV gate: `cargo +1.85.0 check --workspace --all-features` (CI matrix runs stable + 1.85.0)
- Regenerate trybuild snapshots: `TRYBUILD=overwrite cargo test -p vivarium-macros --test ui`
- `cargo test -p vivarium-db` without features compiles **zero** integration tests (all driver-gated); use `--features sqlite,postgres`

## Code Conventions & Common Patterns

- **Lint gates (enforced by CI):** `#![deny(missing_docs)]` in all six crates — every public item needs a doc comment. `#![forbid(unsafe_code)]` in core/db/macros only; web's sole `unsafe` is the pin-projection in `cache.rs` (with SAFETY comment)
- **No-panic rule:** no `unwrap()` in library code (`expect` only in tests); `serve()` returns `io::Result` instead of panicking
- **Error handling:** db exposes `pub type Error = sqlx::Error` (pure passthrough); web/config use `thiserror` 2. No `anyhow` in src. Web converts at call sites: `.map_err(|e| ApiError::Validation(e.to_string()))` — no blanket `From` impls. `ApiError` wire contract: `{"code","message"}`; the internal `system` field is added **only** when `Internal` AND debug mode (`VIVARIUM_DEBUG=1` env or `set_debug_mode(true)`)
- **Column-name safety:** `Query`/`Sorter` take `impl Column` (enum impls with `fn name(&self) -> &'static str`) — never strings; the stringly-typed path is impossible by construction. Identifiers are quoted per driver (`"` SQLite/PG, backtick MySQL) at splice time, so SQL-keyword names (`order`, `desc`) are safe; `Column` impls carry logical names only
- **`#[derive(Entity)]`:** default anchor crate is `::vivarium_rs` — code outside the facade must add `#[entity(crate = "vivarium_db")]`. Attributes: `table`, `crate`, `id` (must be `i64`), `rename`, `json`, `skip`
- **Async:** db helpers take generic `impl Executor<'q, Database = DB>`; pass `&pool` (`paginate` needs `E: Copy`). Web extractors are hand-written `FromRequest`/`FromRequestParts` impls with `Rejection = ApiError`. Driver execution lives in per-driver impls with deliberately boxed futures (`Pin<Box<dyn Future + Send>>`): `impl Future` (RPITIT) leaks the `E: Executor` obligation into handler futures and breaks axum `Send` generalization (rustc #100013) — see `driver.rs` rationale before "optimizing"
- **Sealing/glue:** `mod private { pub trait Sealed {} }` for sealed traits; internal-but-public bounds re-exported `#[doc(hidden)]`
- **Feature gating:** driver impls behind `#[cfg(feature = "sqlite")]` etc.; facade uses `dep:` optional deps. `vivarium-db` has `default = []` (no driver); `vivarium-rs` default = web+config+db+sqlite+postgres (no mysql)
- **Docs:** `//!` crate docs; no `# Examples` sections anywhere — examples live in crate-level `no_run` doctests or on key items. Intra-doc links: bare `[`sqlx`]` for direct deps, reference-style docs.rs links for sibling crates
- **Tests:** inline `#[cfg(test)] mod tests` at file bottom; plain `assert!`/`assert_eq!` (no custom assertion crates); web tests drive `Router::oneshot` via `tower::ServiceExt` (no sockets); db tests build per-file `pool()` = `SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:")` + `MIGRATOR.run` (`max_connections(1)` is mandatory for `sqlite::memory:`); driver-specific test files carry `#![cfg(feature = "sqlite")]` headers

## Important Files

- `Cargo.toml` — workspace manifest (`workspace.package`; note crate versions are NOT inherited: core/db/web/config/rs = 0.1.1, macros = 0.1.2)
- `crates/vivarium-db/src/lib.rs` — `MIGRATOR` static, re-export surface
- `crates/vivarium-db/src/{query,crud,driver}.rs` — query builder, CRUD, driver glue
- `crates/vivarium-web/src/{error,varser,jwt,cache,serve}.rs` — `ApiError` contract, extractor pipeline, JWT, Cache-Control
- `crates/vivarium-config/src/lib.rs` — single-module crate; reload/watch semantics
- `crates/vivarium-rs/src/lib.rs` — facade re-export matrix + feature definitions
- `.github/workflows/ci.yml` — full gate (fmt, clippy `-D warnings`, test, doc, `SQLX_OFFLINE=true` check; matrix stable + 1.85.0; postgres:16 service)
- `README.md` — quick start; `docs/go-rust-mapping.md` — Go→Rust porting map (non-ports rationale)
- `crates/vivarium-db/migrations/0001_create_users.sql` (+ `.down.sql`) — example migration, consumed at compile time

## Runtime/Tooling Preferences

- **Runtime/toolchain:** stable Rust with **MSRV 1.85** (edition 2024 requires it). No `rust-toolchain.toml` — CI matrix pin `1.85.0` is the enforcement. Trybuild `.stderr` snapshots must match both stable and 1.85.0
- **Style:** default rustfmt (no `rustfmt.toml`), default clippy (no `clippy.toml`) at `-D warnings`
- **Key deps (pinned in Cargo.lock):** sqlx 0.8.6 (no MSSQL), axum 0.8.9, garde 0.22, jsonwebtoken 10 (`rust_crypto` — pure Rust, no OpenSSL), thiserror 2, tokio 1.53, tower 0.5, figment 0.10, notify 8, arc-swap 1
- **sqlx offline mode:** CI runs `SQLX_OFFLINE=true cargo check`. No `query!`-family macros exist yet (no `.sqlx/` cache committed). Adding compile-time-checked queries requires `cargo sqlx prepare` + committing `.sqlx/`, or the offline step breaks
- No docker-compose, no `.env*`, no publish/release workflow, no dependabot

## Testing & QA

- **Command:** `cargo test --workspace --all-features` (works without Postgres — `pg.rs` self-skips via `std::env::var("DATABASE_URL")` + eprintln + return). Doctests run automatically; the two crate-level examples are `no_run` (compile-only)
- **Layout:** integration tests in `crates/*/tests/` (db, macros, rs) + inline `#[cfg(test)]` modules (web ×4, config, core, macros). No shared test-utils module — self-contained helpers per file (`pool()`, `drive()`, `body_json()`); duplication is accepted
- **Style to copy:** web handler test = build `Router` + `app.oneshot(req)` (`crates/vivarium-web/src/cache.rs` is the shortest); db test = `#[derive(sqlx::FromRow, Entity)]` + in-memory pool + `MIGRATOR.run` (`crates/vivarium-db/tests/crud.rs`); end-to-end acceptance template = `crates/vivarium-rs/tests/quickstart.rs`; macro behavior = `crates/vivarium-macros/tests/derive.rs`; config reload = `tempfile` + `#[tokio::test(flavor = "multi_thread")]`
- **Coverage:** none — no tarpaulin/llvm-cov/codecov config. The gate is CI: fmt + clippy `-D warnings` + full test suite on stable and 1.85.0
