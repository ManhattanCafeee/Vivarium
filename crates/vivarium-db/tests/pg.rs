//! PostgreSQL integration test, run only when `DATABASE_URL` is set (CI
//! provides a service container). Skipped locally without one.
#![cfg(feature = "postgres")]

use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use vivarium_db::{Order, Pagination, Query, Sorter, create, delete, find_by_id, update_by_id};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "vivarium_users", crate = "vivarium_db")]
struct User {
    id: i64,
    name: String,
    age: i32,
    active: bool,
    profile: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserCol {
    Age,
}

impl vivarium_db::Column for UserCol {
    fn name(&self) -> &'static str {
        match self {
            UserCol::Age => "age",
        }
    }
}

#[tokio::test]
async fn postgres_crud_round_trip() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS vivarium_users (
            id BIGSERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            age INTEGER NOT NULL DEFAULT 0,
            active BOOLEAN NOT NULL DEFAULT FALSE,
            profile JSONB
        )",
    )
    .execute(&pool)
    .await
    .expect("ddl");
    sqlx::query("DELETE FROM vivarium_users")
        .execute(&pool)
        .await
        .expect("cleanup");

    let user = User {
        id: 0,
        name: "pg".to_owned(),
        age: 7,
        active: true,
        profile: json!({"k": [1, 2]}),
    };
    let id = create(&pool, user).await.expect("create");
    assert!(id >= 1);

    let found = find_by_id::<User, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(
        (found.name.as_str(), found.age, found.active),
        ("pg", 7, true)
    );
    assert_eq!(found.profile, json!({"k": [1, 2]}));

    let rows = update_by_id(
        &pool,
        id,
        User {
            id: 0,
            name: "pg2".to_owned(),
            age: 8,
            active: false,
            profile: json!(null),
        },
    )
    .await
    .expect("update");
    assert_eq!(rows, 1);

    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(1, 10), &pool)
        .await
        .expect("paginate");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].name, "pg2");

    assert_eq!(delete::<User, _>(&pool, id).await.expect("delete"), 1);
}
