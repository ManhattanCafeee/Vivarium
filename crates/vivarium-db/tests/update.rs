//! Partial-column [`Update`] tests on SQLite in-memory databases.
#![cfg(feature = "sqlite")]

use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_db::{Column, Expr, Update, create, find_by_id};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "items", crate = "vivarium_db")]
struct Item {
    id: i64,
    name: String,
    qty: i32,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemCol {
    Name,
    Qty,
    UpdatedAt,
}

impl Column for ItemCol {
    fn name(&self) -> &'static str {
        match self {
            ItemCol::Name => "name",
            ItemCol::Qty => "qty",
            ItemCol::UpdatedAt => "updated_at",
        }
    }
}

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool");
    sqlx::query(
        "CREATE TABLE items (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT    NOT NULL,
            qty        INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT
        )",
    )
    .execute(&pool)
    .await
    .expect("schema");
    pool
}

async fn seed(pool: &SqlitePool, name: &str, qty: i32) -> i64 {
    create(
        pool,
        Item {
            id: 0,
            name: name.to_owned(),
            qty,
            updated_at: None,
        },
    )
    .await
    .expect("seed")
}

#[tokio::test]
async fn set_updates_only_the_named_columns() {
    let pool = pool().await;
    let id = seed(&pool, "widget", 1).await;
    let other = seed(&pool, "gadget", 5).await;

    let rows = Update::<Item, ItemCol>::new(id)
        .set(ItemCol::Name, "renamed")
        .set(ItemCol::Qty, 7_i64)
        .execute(&pool)
        .await
        .expect("update");
    assert_eq!(rows, 1);

    let found = find_by_id::<Item, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    assert_eq!((found.name.as_str(), found.qty), ("renamed", 7));

    // the untouched row keeps its values
    let untouched = find_by_id::<Item, _>(&pool, other)
        .await
        .expect("find")
        .expect("row");
    assert_eq!((untouched.name.as_str(), untouched.qty), ("gadget", 5));
}

#[tokio::test]
async fn set_expr_now_writes_a_timestamp() {
    let pool = pool().await;
    let id = seed(&pool, "widget", 1).await;
    sqlx::query("UPDATE items SET updated_at = '1970-01-01 00:00:00' WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("seed timestamp");

    let rows = Update::<Item, ItemCol>::new(id)
        .set_expr(ItemCol::UpdatedAt, Expr::Now)
        .execute(&pool)
        .await
        .expect("update");
    assert_eq!(rows, 1);

    let found = find_by_id::<Item, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    let updated_at = found.updated_at.expect("CURRENT_TIMESTAMP is not NULL");
    let parsed = chrono::NaiveDateTime::parse_from_str(&updated_at, "%Y-%m-%d %H:%M:%S")
        .expect("CURRENT_TIMESTAMP text");
    assert!(parsed.and_utc().timestamp() > 1_000_000_000, "{updated_at}");
}

#[tokio::test]
async fn update_of_a_missing_row_affects_zero() {
    let pool = pool().await;
    let rows = Update::<Item, ItemCol>::new(999_i64)
        .set(ItemCol::Name, "nobody")
        .execute(&pool)
        .await
        .expect("update");
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn update_without_any_set_is_a_protocol_error() {
    let pool = pool().await;
    let id = seed(&pool, "widget", 1).await;
    match Update::<Item, ItemCol>::new(id).execute(&pool).await {
        Err(sqlx::Error::Protocol(message)) => assert!(!message.is_empty()),
        other => panic!("expected a protocol error, got {other:?}"),
    }
}
