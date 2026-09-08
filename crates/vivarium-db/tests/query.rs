//! Query builder tests: where/order/limit/offset combinations, count, and
//! pagination on SQLite in-memory databases, plus SQL-text unit tests that
//! need no database.
#![cfg(feature = "sqlite")]

use serde_json::json;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_db::{Column, Order, Pagination, Query, Sorter, create};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserCol {
    Name,
    Age,
    Active,
}

impl Column for UserCol {
    fn name(&self) -> &'static str {
        match self {
            UserCol::Name => "name",
            UserCol::Age => "age",
            UserCol::Active => "active",
        }
    }
}

fn user(name: &str, age: i32, active: bool) -> User {
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

async fn seed(pool: &SqlitePool, n: i32) {
    for age in 1..=n {
        create(pool, user(&format!("user{age}"), age, age % 2 == 0))
            .await
            .expect("seed");
    }
}

#[tokio::test]
async fn where_order_limit_offset() {
    let pool = pool().await;
    seed(&pool, 10).await; // active = even ages: 2, 4, 6, 8, 10
    let rows = Query::<_, User>::new()
        .where_eq(UserCol::Active, true)
        .order_by(Sorter::new(UserCol::Age, Order::Desc))
        .limit(3)
        .offset(1)
        .find(&pool)
        .await
        .expect("find");
    let ages: Vec<i32> = rows.iter().map(|u| u.age).collect();
    assert_eq!(ages, vec![8, 6, 4]);
}

#[tokio::test]
async fn multiple_wheres_combine_with_and() {
    let pool = pool().await;
    seed(&pool, 10).await;
    let rows = Query::<_, User>::new()
        .where_eq(UserCol::Active, true)
        .where_eq(UserCol::Age, 6_i64)
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "user6");
}

#[tokio::test]
async fn first_returns_top_row() {
    let pool = pool().await;
    seed(&pool, 10).await;
    let first = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Desc))
        .first(&pool)
        .await
        .expect("first")
        .expect("row");
    assert_eq!(first.age, 10);

    let none = Query::<_, User>::new()
        .where_eq(UserCol::Age, 999_i64)
        .first(&pool)
        .await
        .expect("first");
    assert!(none.is_none());
}

#[tokio::test]
async fn query_count_respects_wheres() {
    let pool = pool().await;
    seed(&pool, 10).await;
    let n = Query::<_, User>::new()
        .where_eq(UserCol::Active, false)
        .count(&pool)
        .await
        .expect("count");
    assert_eq!(n, 5); // odd ages 1, 3, 5, 7, 9
}

#[tokio::test]
async fn paginate_returns_total_and_page() {
    let pool = pool().await;
    seed(&pool, 45).await;
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(2, 10), &pool)
        .await
        .expect("paginate");
    assert_eq!((page.total, page.page, page.size), (45, 2, 10));
    assert_eq!(page.pages(), 5);
    assert_eq!(page.content.len(), 10);
    assert_eq!(page.content[0].age, 11);
    assert_eq!(page.content[9].age, 20);
}

#[tokio::test]
async fn paginate_normalizes_out_of_range_sizes() {
    let pool = pool().await;
    seed(&pool, 45).await;

    // size 0 → 20, page 0 → 1
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(0, 0), &pool)
        .await
        .expect("paginate");
    assert_eq!((page.page, page.size), (1, 20));
    assert_eq!(page.content.len(), 20);
    assert_eq!(page.total, 45);

    // size 101 → 100
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(1, 101), &pool)
        .await
        .expect("paginate");
    assert_eq!((page.page, page.size), (1, 100));
    assert_eq!(page.content.len(), 45);

    // page beyond the data → empty content, correct total
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(4, 20), &pool)
        .await
        .expect("paginate");
    assert_eq!(page.total, 45);
    assert!(page.content.is_empty());
}

#[tokio::test]
async fn paginate_replaces_query_limit_offset() {
    let pool = pool().await;
    seed(&pool, 10).await;

    // The query's own limit/offset are replaced by the pagination's; a
    // stale duplicate LIMIT clause would be a syntax error here.
    let page = Query::<_, User>::new()
        .limit(1)
        .offset(1)
        .paginate(Pagination::new(1, 20), &pool)
        .await
        .expect("paginate");
    assert_eq!(page.total, 10);
    assert_eq!(page.content.len(), 10);
}

#[tokio::test]
async fn json_column_query_round_trip() {
    let pool = pool().await;
    let mut user = user("jsonfan", 1, true);
    user.profile = json!({"langs": ["rust"], "n": 2});
    create(&pool, user).await.expect("create");
    let rows = Query::<_, User>::new()
        .where_eq(UserCol::Name, "jsonfan".to_owned())
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].profile, json!({"langs": ["rust"], "n": 2}));
}

#[test]
fn sql_text_is_generated_correctly() {
    let query = Query::<sqlx::Sqlite, User>::new()
        .where_eq(UserCol::Age, 5_i64)
        .where_eq(UserCol::Active, true)
        .order_by(Sorter::new(UserCol::Name, Order::Desc))
        .limit(10)
        .offset(20);
    assert_eq!(
        query.sql(),
        "SELECT * FROM \"users\" WHERE \"age\" = ? AND \"active\" = ? ORDER BY \"name\" DESC LIMIT 10 OFFSET 20"
    );
}

#[cfg(feature = "postgres")]
#[test]
fn sql_text_uses_dollar_placeholders() {
    let query = Query::<sqlx::Postgres, User>::new()
        .where_eq(UserCol::Age, 5_i64)
        .where_eq(UserCol::Active, true);
    assert_eq!(
        query.sql(),
        "SELECT * FROM \"users\" WHERE \"age\" = $1 AND \"active\" = $2"
    );
}
