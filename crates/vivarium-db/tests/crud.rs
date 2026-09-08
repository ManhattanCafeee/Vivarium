//! CRUD round-trip tests on SQLite in-memory databases.
#![cfg(feature = "sqlite")]

use serde_json::json;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_db::{count, create, delete, exists, find_by_id, update_by_id};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "users", crate = "vivarium_db")]
struct User {
    id: i64,
    name: String,
    email: Option<String>,
    age: i32,
    active: bool,
    profile: serde_json::Value,
}

fn new_user(name: &str, age: i32, active: bool) -> User {
    User {
        id: 0,
        name: name.to_owned(),
        email: None,
        age,
        active,
        profile: json!({}),
    }
}

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool");
    vivarium_db::MIGRATOR.run(&pool).await.expect("migrate");
    pool
}

#[tokio::test]
async fn crud_round_trip() {
    let pool = pool().await;

    // id 0 → autoincrement
    let id = create(&pool, new_user("ada", 30, true))
        .await
        .expect("create");
    assert!(id >= 1);

    let found = find_by_id::<User, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(
        (found.name.as_str(), found.age, found.active),
        ("ada", 30, true)
    );

    let rows = update_by_id(&pool, id, new_user("grace", 40, false))
        .await
        .expect("update");
    assert_eq!(rows, 1);
    let found = find_by_id::<User, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(
        (found.name.as_str(), found.age, found.active),
        ("grace", 40, false)
    );

    let rows = delete::<User, _>(&pool, id).await.expect("delete");
    assert_eq!(rows, 1);
    assert!(
        find_by_id::<User, _>(&pool, id)
            .await
            .expect("find")
            .is_none()
    );
}

#[tokio::test]
async fn create_with_explicit_id() {
    let pool = pool().await;
    let mut user = new_user("linus", 55, true);
    user.id = 42;
    let id = create(&pool, user).await.expect("create");
    assert_eq!(id, 42);
    let found = find_by_id::<User, _>(&pool, 42)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(found.name, "linus");
}

#[tokio::test]
async fn count_and_exists() {
    let pool = pool().await;
    for (name, age) in [("a", 1), ("b", 2), ("c", 3)] {
        create(&pool, new_user(name, age, true))
            .await
            .expect("seed");
    }
    assert_eq!(count::<User, _>(&pool).await.expect("count"), 3);
    assert!(exists::<User, _>(&pool, 1).await.expect("exists"));
    assert!(!exists::<User, _>(&pool, 10_000).await.expect("exists"));
}

#[tokio::test]
async fn update_and_delete_of_missing_rows_affect_zero() {
    let pool = pool().await;
    assert_eq!(
        update_by_id(&pool, 999, new_user("nobody", 1, false))
            .await
            .expect("update"),
        0
    );
    assert_eq!(delete::<User, _>(&pool, 999).await.expect("delete"), 0);
}

#[tokio::test]
async fn json_column_round_trip() {
    let pool = pool().await;
    let mut user = new_user("jsonfan", 1, true);
    user.profile = json!({"langs": ["rust", "go"], "score": 9.5});
    let id = create(&pool, user).await.expect("create");
    let found = find_by_id::<User, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(
        found.profile,
        json!({"langs": ["rust", "go"], "score": 9.5})
    );
}
