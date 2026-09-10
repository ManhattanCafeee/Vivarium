//! MySQL integration test, run only when `MYSQL_DATABASE_URL` is set (CI
//! provides a service container alongside PostgreSQL's `DATABASE_URL`).
//! Skipped locally without one.
#![cfg(feature = "mysql")]

use serde_json::json;
use sqlx::mysql::MySqlPoolOptions;
use vivarium_db::{
    Column, Expr, Order, Pagination, Predicate, Query, Sorter, Update, create, delete, find_by_id,
    is_unique_violation, update_by_id, with_transaction,
};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "vivarium_mysql_users", crate = "vivarium_db")]
struct User {
    id: u64,
    name: String,
    email: Option<String>,
    age: i32,
    active: bool,
    profile: serde_json::Value,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserCol {
    Name,
    Age,
    CreatedAt,
}

impl Column for UserCol {
    fn name(&self) -> &'static str {
        match self {
            UserCol::Name => "name",
            UserCol::Age => "age",
            UserCol::CreatedAt => "created_at",
        }
    }
}

fn user(name: &str, email: &str, age: i32) -> User {
    User {
        id: 0,
        name: name.to_owned(),
        email: Some(email.to_owned()),
        age,
        active: true,
        profile: json!({}),
        created_at: None,
    }
}

#[tokio::test]
async fn mysql_crud_query_update_transaction_and_round_trips() {
    let Ok(url) = std::env::var("MYSQL_DATABASE_URL") else {
        eprintln!("skipping: MYSQL_DATABASE_URL not set");
        return;
    };
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect");

    sqlx::query("DROP TABLE IF EXISTS vivarium_mysql_users")
        .execute(&pool)
        .await
        .expect("drop stale table");
    sqlx::query(
        "CREATE TABLE vivarium_mysql_users (
            id         BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
            name       VARCHAR(255)    NOT NULL,
            email      VARCHAR(255)    NULL,
            age        INT             NOT NULL DEFAULT 0,
            active     BOOLEAN         NOT NULL DEFAULT FALSE,
            profile    JSON            NULL,
            created_at DATETIME(6)     NULL,
            UNIQUE KEY vivarium_mysql_users_email_unique (email)
        )",
    )
    .execute(&pool)
    .await
    .expect("ddl");

    // --- CRUD, JSON, and DateTime round-trips ---
    let created_at = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp");
    let mut row = user("mysql", "mysql@example.com", 7);
    row.profile = json!({"k": [1, 2]});
    row.created_at = Some(created_at);

    let id = create(&pool, row.clone()).await.expect("create");
    assert!(id >= 1);

    let found = find_by_id::<User, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(
        (found.name.as_str(), found.age, found.active),
        ("mysql", 7, true)
    );
    assert_eq!(found.profile, json!({"k": [1, 2]}));
    assert_eq!(found.created_at, Some(created_at));

    let rows = update_by_id(
        &pool,
        id,
        User {
            name: "mysql2".to_owned(),
            ..row.clone()
        },
    )
    .await
    .expect("update");
    assert_eq!(rows, 1);

    // --- Query filter, DateTime bind ---
    let filtered = Query::<_, User>::new()
        .filter(Predicate::eq(UserCol::Name, "mysql2"))
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(filtered.len(), 1);

    let by_timestamp = Query::<_, User>::new()
        .where_eq(UserCol::CreatedAt, created_at)
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(by_timestamp.len(), 1);
    assert_eq!(by_timestamp[0].id, id);

    // --- paginate ---
    for (name, email, age) in [
        ("a", "a@example.com", 1),
        ("b", "b@example.com", 2),
        ("c", "c@example.com", 3),
    ] {
        create(&pool, user(name, email, age)).await.expect("seed");
    }
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(1, 2), &pool)
        .await
        .expect("paginate");
    assert_eq!(page.total, 4);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].age, 1);

    // --- Update: bound value plus a fixed expression ---
    let rows = Update::<User, UserCol>::new(id)
        .set(UserCol::Age, 99_i64)
        .set_expr(UserCol::CreatedAt, Expr::Now)
        .execute(&pool)
        .await
        .expect("update");
    assert_eq!(rows, 1);
    let found = find_by_id::<User, _>(&pool, id)
        .await
        .expect("find")
        .expect("row");
    assert_eq!(found.age, 99);
    assert_ne!(found.created_at, Some(created_at));

    // --- transaction commit ---
    let (first, second) = with_transaction(&pool, async |tx| {
        let first = create(&mut **tx, user("tx-a", "tx-a@example.com", 20)).await?;
        let second = create(&mut **tx, user("tx-b", "tx-b@example.com", 21)).await?;
        Ok((first, second))
    })
    .await
    .expect("commit");
    assert!(
        find_by_id::<User, _>(&pool, first)
            .await
            .expect("find")
            .is_some()
    );
    assert!(
        find_by_id::<User, _>(&pool, second)
            .await
            .expect("find")
            .is_some()
    );

    // --- transaction rollback ---
    let rolled_back = with_transaction(&pool, async |tx| {
        create(&mut **tx, user("tx-c", "tx-c@example.com", 30)).await?;
        sqlx::query("INSERT INTO vivarium_mysql_users (name) VALUES (NULL)")
            .execute(&mut **tx)
            .await?;
        Ok(())
    })
    .await;
    assert!(rolled_back.is_err());
    assert!(
        Query::<_, User>::new()
            .where_eq(UserCol::Name, "tx-c")
            .find(&pool)
            .await
            .expect("find")
            .is_empty()
    );

    // --- unique violation: MySQL 1062 ---
    let err = create(&pool, user("dup", "mysql@example.com", 1))
        .await
        .expect_err("duplicate email must fail");
    assert!(is_unique_violation(&err), "not a unique violation: {err:?}");
    let mysql_error = err
        .as_database_error()
        .expect("database error")
        .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
        .expect("mysql database error");
    assert_eq!(mysql_error.number(), 1062);

    assert_eq!(delete::<User, _>(&pool, id).await.expect("delete"), 1);
    sqlx::query("DROP TABLE vivarium_mysql_users")
        .execute(&pool)
        .await
        .expect("drop");
}
