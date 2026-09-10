# vivarium-db

Type-safe database ergonomics on top of [sqlx](https://github.com/launchbadge/sqlx):
generic CRUD for `Entity` types, a chainable query builder with typed filters,
compile-time-checked bind values, partial-column updates, transactions, and
pagination.

## Features

| Feature | Driver |
|---|---|
| `sqlite` | SQLite |
| `postgres` | PostgreSQL |
| `mysql` | MySQL (enables sqlx's `mysql-rsa`, needed for non-TLS local servers) |

No drivers are enabled by default. The SQL text is driver-portable
(`?` vs `$1` handled per driver); enable whichever drivers you use.

## Quick start

```rust,ignore
use sqlx::sqlite::SqlitePool;
use vivarium_db::{create, find_by_id, Query};

#[derive(Clone, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "users", crate = "vivarium_db")]
struct User {
    id: i64,
    name: String,
}

async fn example(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let id = create(pool, User { id: 0, name: "ada".into() }).await?;
    let user = find_by_id::<User, _>(pool, id).await?;
    let all = Query::<_, User>::new().find(pool).await?;
    let _ = (user, all);
    Ok(())
}
```

- **Queries**: `where_eq`, `filter(Predicate)`, `raw_where(RawFragment)`,
  `select`, `order_by`, `limit`, `offset`, plus `find`, `first`, `count`,
  `paginate`, `paginate_with_total`, and a `sql()` renderer for debugging.
  Column names can only come from a `Column` enum — SQL injection via
  sort/filter columns is impossible at compile time, and every value is bound.
- **Predicates** (`Predicate::eq/ne/lt/le/gt/ge/like/one_of/is_null/and/or/…`)
  build `AND`/`OR`/`NOT` trees; `starts_with`/`ends_with`/`contains` escape
  `%`/`_` so the value matches literally.
- **Partial updates**: `Update::<User, UserCol>::new(id).set(UserCol::Name, "ada")`
  or `.set_expr(UserCol::UpdatedAt, Expr::Now)`.
- **Transactions**: `with_transaction(pool, async |tx| { … })` commits on `Ok`
  and rolls back on `Err`. The closure must be an *async* closure (it holds the
  borrowed transaction across `await`), and `&mut **tx` is the executor for
  every helper inside it.
- **Primary keys** are typed: `i64`, `u64`, `i32`, `u32`, or `String`.
  A `u64` key above `i64::MAX` is an error instead of a silent wrap.
- **Generic helpers**: `create`, `update_by_id` (writes every non-id column),
  `delete`, `find_by_id`, `count`, `exists`, and `is_unique_violation(&sqlx::Error)`
  — driver-agnostic (MySQL 1062, PostgreSQL 23505, SQLite constraint codes)
  detection of unique/primary-key violations. The web layer maps it to
  `ApiError::Conflict` (409).

## Migrations

This crate ships **no** embedded migrator on purpose: a library-owned
`_sqlx_migrations` table collides with the host application's, and a
SQLite-flavoured example schema would pollute a MySQL or PostgreSQL database.
Use `sqlx::migrate!("./migrations")` in the application; see
`examples/migrations/` for the sample layout the test suite also uses.

MSRV: Rust 1.94.
