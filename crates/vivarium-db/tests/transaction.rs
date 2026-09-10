//! [`with_transaction`] commit/rollback tests on SQLite in-memory databases.
#![cfg(feature = "sqlite")]

use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_db::{count, create, find_by_id, with_transaction};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "accounts", crate = "vivarium_db")]
struct Account {
    id: i64,
    name: String,
}

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool");
    sqlx::query("CREATE TABLE accounts (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .expect("schema");
    pool
}

/// A plain executor-taking helper, to pin that a function called with
/// `&mut **tx` (the driver connection behind the transaction) works inside
/// the closure.
async fn insert_account<'e, E>(db: E, name: &str) -> Result<i64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite> + 'e,
{
    create(
        db,
        Account {
            id: 0,
            name: name.to_owned(),
        },
    )
    .await
}

#[tokio::test]
async fn commit_makes_both_inserts_visible() {
    let pool = pool().await;

    let (first, second) = with_transaction(&pool, async |tx| {
        let first = insert_account(&mut **tx, "first").await?;
        let second = insert_account(&mut **tx, "second").await?;
        Ok((first, second))
    })
    .await
    .expect("commit");

    assert_eq!((first, second), (1, 2));
    assert_eq!(count::<Account, _>(&pool).await.expect("count"), 2);
    let found = find_by_id::<Account, _>(&pool, second)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(found.name, "second");
}

#[tokio::test]
async fn rollback_discards_the_first_insert() {
    let pool = pool().await;

    let result = with_transaction(&pool, async |tx| {
        insert_account(&mut **tx, "first").await?;
        // `name` is NOT NULL: this statement fails, so the batch rolls back.
        sqlx::query("INSERT INTO accounts (name) VALUES (NULL)")
            .execute(&mut **tx)
            .await?;
        Ok(())
    })
    .await;

    assert!(result.is_err());
    assert_eq!(count::<Account, _>(&pool).await.expect("count"), 0);
}
