//! Migration up/down tests against SQLite in-memory databases.
#![cfg(feature = "sqlite")]

use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

async fn pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool")
}

async fn users_table_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'users'")
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn migrations_apply_and_undo() {
    let pool = pool().await;

    assert_eq!(users_table_count(&pool).await, 0);
    vivarium_db::MIGRATOR.run(&pool).await.expect("migrate up");
    assert_eq!(users_table_count(&pool).await, 1);

    // the migrated table is usable
    sqlx::query("INSERT INTO users (name) VALUES ('migrated')")
        .execute(&pool)
        .await
        .expect("insert");

    vivarium_db::MIGRATOR
        .undo(&pool, 0)
        .await
        .expect("migrate down");
    assert_eq!(users_table_count(&pool).await, 0);
}
