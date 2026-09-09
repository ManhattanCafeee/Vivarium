# vivarium-db

Type-safe database ergonomics on top of [sqlx](https://github.com/launchbadge/sqlx):
generic CRUD for `Entity` types, a chainable query builder with
compile-time-checked bind values, and pagination.

## Features

| Feature | Driver |
|---|---|
| `sqlite` | SQLite |
| `postgres` | PostgreSQL |
| `mysql` | MySQL |

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

`Query` also offers `first`, `count`, `paginate`, `where_eq`,
`order_by`, `limit`, `offset`, and a `sql()` renderer for debugging.
Column names can only come from a `Column` enum — SQL injection via
sort/where columns is impossible at compile time.

Generic helpers included: `create`, `update_by_id`, `delete`,
`find_by_id`, `count`, `exists`, plus
`is_unique_violation(&sqlx::Error)` — driver-agnostic (MySQL 1062,
PostgreSQL 23505, SQLite constraint codes) detection of unique/primary-key
violations, the standard "pre-check then insert" race fallback, which the
web layer maps to `ApiError::Conflict` (409). And an example migrator
(`vivarium_db::MIGRATOR`) applied with `sqlx::migrate!`.

MSRV: Rust 1.85.
