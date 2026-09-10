//! Primary-key round-trip tests on SQLite in-memory databases: explicit and
//! database-generated ids, `String` keys, and keys that do not fit in `i64`.
#![cfg(feature = "sqlite")]

use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_db::{count, create, find_by_id};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "auto_rows", crate = "vivarium_db")]
struct AutoRow {
    id: i64,
    name: String,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "big_rows", crate = "vivarium_db")]
struct BigRow {
    id: u64,
    name: String,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "tokens", crate = "vivarium_db")]
struct Token {
    #[entity(id)]
    key: String,
    label: String,
}

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool");
    for ddl in [
        "CREATE TABLE auto_rows (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)",
        "CREATE TABLE big_rows (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
        "CREATE TABLE tokens (key TEXT PRIMARY KEY, label TEXT NOT NULL)",
    ] {
        sqlx::query(ddl).execute(&pool).await.expect("schema");
    }
    pool
}

#[tokio::test]
async fn u64_key_round_trips_with_an_explicit_value() {
    let pool = pool().await;
    let id = create(
        &pool,
        BigRow {
            id: 42,
            name: "explicit".to_owned(),
        },
    )
    .await
    .expect("create");
    assert_eq!(id, 42);

    let found = find_by_id::<BigRow, _>(&pool, 42)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(found.name, "explicit");

    // The largest representable key still round-trips.
    let max = i64::MAX as u64;
    assert_eq!(
        create(
            &pool,
            BigRow {
                id: max,
                name: "max".to_owned(),
            },
        )
        .await
        .expect("create"),
        max
    );
    let found = find_by_id::<BigRow, _>(&pool, max)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(found.name, "max");
}

#[tokio::test]
async fn string_key_round_trips() {
    let pool = pool().await;
    let key = create(
        &pool,
        Token {
            key: "tok-1".to_owned(),
            label: "first".to_owned(),
        },
    )
    .await
    .expect("create");
    assert_eq!(key, "tok-1");

    let found = find_by_id::<Token, _>(&pool, "tok-1".to_owned())
        .await
        .expect("find")
        .expect("row");
    assert_eq!(found.label, "first");
}

#[tokio::test]
async fn unset_i64_key_is_generated_by_the_database() {
    let pool = pool().await;
    let first = create(
        &pool,
        AutoRow {
            id: 0,
            name: "a".to_owned(),
        },
    )
    .await
    .expect("create");
    let second = create(
        &pool,
        AutoRow {
            id: 0,
            name: "b".to_owned(),
        },
    )
    .await
    .expect("create");

    assert_eq!((first, second), (1, 2));
    let found = find_by_id::<AutoRow, _>(&pool, second)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(found.name, "b");
}

#[tokio::test]
async fn key_above_i64_max_is_an_error_not_a_wrap() {
    let pool = pool().await;
    let too_big = u64::MAX;

    let err = create(
        &pool,
        BigRow {
            id: too_big,
            name: "nope".to_owned(),
        },
    )
    .await
    .expect_err("create must reject a key above i64::MAX");
    assert!(matches!(err, sqlx::Error::Encode(_)), "got {err:?}");

    let err = find_by_id::<BigRow, _>(&pool, too_big)
        .await
        .expect_err("find must reject a key above i64::MAX");
    assert!(matches!(err, sqlx::Error::Encode(_)), "got {err:?}");

    // nothing was written
    assert_eq!(count::<BigRow, _>(&pool).await.expect("count"), 0);
}
