//! Query builder tests: where/order/limit/offset combinations, projection,
//! typed predicates, raw fragments, count, and pagination on SQLite
//! in-memory databases, plus SQL-text unit tests that need no database.
#![cfg(feature = "sqlite")]

use serde_json::json;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_db::{
    Column, Order, Pagination, Predicate, Query, RawFragment, Sorter, Value, create,
};

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

/// A deliberately narrowed projection target: only `name` is selected, and
/// `name` doubles as the (String) primary key so the struct can derive
/// `Entity` while decoding from a one-column `SELECT`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, vivarium_db::Entity)]
#[entity(table = "users", crate = "vivarium_db")]
struct UserName {
    #[entity(id)]
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserCol {
    Name,
    Email,
    Age,
    Active,
}

impl Column for UserCol {
    fn name(&self) -> &'static str {
        match self {
            UserCol::Name => "name",
            UserCol::Email => "email",
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

fn user_with_email(name: &str, age: i32, email: Option<&str>) -> User {
    User {
        email: email.map(str::to_owned),
        ..user(name, age, true)
    }
}

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite pool");
    sqlx::migrate!("./examples/migrations")
        .run(&pool)
        .await
        .expect("migrate");
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
async fn first_ignores_its_own_limit_and_offset() {
    let pool = pool().await;
    seed(&pool, 10).await;

    let query = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .limit(5)
        .offset(2);

    // `first()` runs `ORDER BY … LIMIT 1` and ignores the query's own
    // limit/offset, so the executed statement carries exactly one LIMIT
    // clause — the naive `… LIMIT 5 OFFSET 2 LIMIT 1` is invalid SQL and
    // would fail below. `sql()` renders the builder's own clauses, so it
    // shows that single (builder) LIMIT.
    let sql = query.sql();
    assert_eq!(
        sql,
        "SELECT * FROM \"users\" ORDER BY \"age\" ASC LIMIT 5 OFFSET 2"
    );
    assert_eq!(sql.matches("LIMIT").count(), 1, "sql was {sql}");

    // Executing it returns the first row of the ordering, not the third, and
    // must not fail on a duplicated LIMIT clause.
    let first = query.first(&pool).await.expect("first").expect("row");
    assert_eq!(first.age, 1);
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
async fn select_projects_columns_and_decodes_narrowed_rows() {
    let pool = pool().await;
    for name in ["ada", "grace", "linus"] {
        create(&pool, user(name, 1, true)).await.expect("seed");
    }

    let query = Query::<_, UserName>::new().select(&[UserCol::Name]);
    let sql = query.sql();
    assert!(sql.starts_with("SELECT \"name\" FROM"), "sql was {sql}");
    assert_eq!(sql, "SELECT \"name\" FROM \"users\"");

    let mut names: Vec<String> = query
        .find(&pool)
        .await
        .expect("find projected rows")
        .into_iter()
        .map(|row| row.name)
        .collect();
    names.sort();
    assert_eq!(names, ["ada", "grace", "linus"]);
}

#[tokio::test]
async fn filter_executes_eq_in_and_null_checks() {
    let pool = pool().await;
    for (name, age, email) in [
        ("user1", 1, None),
        ("user2", 2, Some("two@example.com")),
        ("user3", 3, None),
        ("user4", 4, Some("four@example.com")),
        ("user7", 7, None),
    ] {
        create(&pool, user_with_email(name, age, email))
            .await
            .expect("seed");
    }

    let rows = Query::<_, User>::new()
        .filter(Predicate::eq(UserCol::Name, "user3"))
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "user3");

    let mut ages: Vec<i32> = Query::<_, User>::new()
        .filter(Predicate::one_of(UserCol::Age, [2_i64, 4, 7]))
        .find(&pool)
        .await
        .expect("find")
        .into_iter()
        .map(|row| row.age)
        .collect();
    ages.sort_unstable();
    assert_eq!(ages, [2, 4, 7]);

    let null = Query::<_, User>::new()
        .filter(Predicate::is_null(UserCol::Email))
        .count(&pool)
        .await
        .expect("count");
    assert_eq!(null, 3);

    let not_null = Query::<_, User>::new()
        .filter(Predicate::is_not_null(UserCol::Email))
        .count(&pool)
        .await
        .expect("count");
    assert_eq!(not_null, 2);
}

#[tokio::test]
async fn filter_escapes_like_wildcards_in_search_values() {
    let pool = pool().await;
    for (name, age) in [("100%", 1), ("1000", 2), ("a_b", 3), ("axb", 4)] {
        create(&pool, user(name, age, true)).await.expect("seed");
    }

    // `%` in the search value is a literal, not a wildcard: an escaped
    // `contains("%")` finds only the row that really contains `%`.
    let rows = Query::<_, User>::new()
        .filter(Predicate::contains(UserCol::Name, "%"))
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "100%");

    // Same for `_`.
    let rows = Query::<_, User>::new()
        .filter(Predicate::contains(UserCol::Name, "_"))
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "a_b");

    // `starts_with` escapes too, so `100%` does not match `1000`.
    let rows = Query::<_, User>::new()
        .filter(Predicate::starts_with(UserCol::Name, "100%"))
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "100%");

    // By contrast `like` passes the pattern through with wildcards active.
    let rows = Query::<_, User>::new()
        .filter(Predicate::like(UserCol::Name, "100%"))
        .find(&pool)
        .await
        .expect("find");
    assert_eq!(rows.len(), 2);
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
    assert_eq!((page.total, page.page, page.per_page), (45, 2, 10));
    assert_eq!(page.pages(), 5);
    assert_eq!(page.items.len(), 10);
    assert_eq!(page.items[0].age, 11);
    assert_eq!(page.items[9].age, 20);
}

#[tokio::test]
async fn paginate_normalizes_out_of_range_sizes() {
    let pool = pool().await;
    seed(&pool, 45).await;

    // per_page 0 → 20, page 0 → 1
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(0, 0), &pool)
        .await
        .expect("paginate");
    assert_eq!((page.page, page.per_page), (1, 20));
    assert_eq!(page.items.len(), 20);
    assert_eq!(page.total, 45);

    // per_page 101 → 100
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(1, 101), &pool)
        .await
        .expect("paginate");
    assert_eq!((page.page, page.per_page), (1, 100));
    assert_eq!(page.items.len(), 45);

    // page beyond the data → empty items, correct total
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate(Pagination::new(4, 20), &pool)
        .await
        .expect("paginate");
    assert_eq!(page.total, 45);
    assert!(page.items.is_empty());
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
    assert_eq!(page.items.len(), 10);
}

#[tokio::test]
async fn paginate_with_total_runs_no_count_query() {
    let pool = pool().await;
    seed(&pool, 3).await;

    // 7 is not the real row count: a COUNT query would report 3 instead.
    let page = Query::<_, User>::new()
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .paginate_with_total(Pagination::new(1, 2), 7, &pool)
        .await
        .expect("paginate");
    assert_eq!(page.total, 7);
    assert_eq!((page.page, page.per_page), (1, 2));
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].age, 1);
    assert_eq!(page.items[1].age, 2);
}

#[tokio::test]
async fn raw_where_binds_values_in_order() {
    let pool = pool().await;
    seed(&pool, 10).await; // active = even ages

    let fragment = RawFragment::new(
        "age > ? AND active = ?",
        vec![Value::I64(7), Value::Bool(true)],
    )
    .expect("one bind per placeholder");
    assert_eq!(fragment.sql(), "age > ? AND active = ?");
    assert_eq!(fragment.binds().len(), 2);

    let rows = Query::<_, User>::new()
        .raw_where(fragment)
        .order_by(Sorter::new(UserCol::Age, Order::Asc))
        .find(&pool)
        .await
        .expect("find");
    let ages: Vec<i32> = rows.iter().map(|u| u.age).collect();
    assert_eq!(ages, vec![8, 10]);
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

#[test]
fn predicate_sql_pins_empty_lists_and_like_escape() {
    fn sql(predicate: Predicate<UserCol>) -> String {
        Query::<sqlx::Sqlite, User>::new().filter(predicate).sql()
    }

    // An empty IN matches nothing, an empty NOT IN matches everything.
    assert_eq!(
        sql(Predicate::one_of(UserCol::Age, Vec::<i64>::new())),
        "SELECT * FROM \"users\" WHERE 1 = 0"
    );
    assert_eq!(
        sql(Predicate::not_one_of(UserCol::Age, Vec::<i64>::new())),
        "SELECT * FROM \"users\" WHERE 1 = 1"
    );

    // An empty AND matches everything, an empty OR matches nothing.
    assert_eq!(
        sql(Predicate::and(Vec::<Predicate<UserCol>>::new())),
        "SELECT * FROM \"users\" WHERE 1 = 1"
    );
    assert_eq!(
        sql(Predicate::or(Vec::<Predicate<UserCol>>::new())),
        "SELECT * FROM \"users\" WHERE 1 = 0"
    );

    // The value-building helpers bind the pattern and append the escape
    // clause; the pattern text never reaches the SQL string.
    assert_eq!(
        sql(Predicate::contains(UserCol::Name, "50%_x")),
        "SELECT * FROM \"users\" WHERE \"name\" LIKE ? ESCAPE '!'"
    );
    assert_eq!(
        sql(Predicate::starts_with(UserCol::Name, "a")),
        "SELECT * FROM \"users\" WHERE \"name\" LIKE ? ESCAPE '!'"
    );
    // `like`/`not_like` keep the caller's wildcards and add no escape clause.
    assert_eq!(
        sql(Predicate::like(UserCol::Name, "50%")),
        "SELECT * FROM \"users\" WHERE \"name\" LIKE ?"
    );
    assert_eq!(
        sql(Predicate::not_like(UserCol::Name, "50%")),
        "SELECT * FROM \"users\" WHERE \"name\" NOT LIKE ?"
    );

    // Every comparison operator keeps its own direction and symbol.
    assert_eq!(
        sql(Predicate::eq(UserCol::Age, 1_i64)),
        "SELECT * FROM \"users\" WHERE \"age\" = ?"
    );
    assert_eq!(
        sql(Predicate::ne(UserCol::Age, 1_i64)),
        "SELECT * FROM \"users\" WHERE \"age\" <> ?"
    );
    assert_eq!(
        sql(Predicate::lt(UserCol::Age, 1_i64)),
        "SELECT * FROM \"users\" WHERE \"age\" < ?"
    );
    assert_eq!(
        sql(Predicate::le(UserCol::Age, 1_i64)),
        "SELECT * FROM \"users\" WHERE \"age\" <= ?"
    );
    assert_eq!(
        sql(Predicate::gt(UserCol::Age, 1_i64)),
        "SELECT * FROM \"users\" WHERE \"age\" > ?"
    );
    assert_eq!(
        sql(Predicate::ge(UserCol::Age, 1_i64)),
        "SELECT * FROM \"users\" WHERE \"age\" >= ?"
    );

    // `ends_with` escapes like `contains`/`starts_with`; `is_not_null` and
    // `negate` keep their own shapes.
    assert_eq!(
        sql(Predicate::ends_with(UserCol::Name, "x")),
        "SELECT * FROM \"users\" WHERE \"name\" LIKE ? ESCAPE '!'"
    );
    assert_eq!(
        sql(Predicate::is_not_null(UserCol::Name)),
        "SELECT * FROM \"users\" WHERE \"name\" IS NOT NULL"
    );
    assert_eq!(
        sql(Predicate::eq(UserCol::Name, "a").negate()),
        "SELECT * FROM \"users\" WHERE NOT (\"name\" = ?)"
    );

    // `one_of` binds each element behind its own placeholder.
    assert_eq!(
        sql(Predicate::one_of(UserCol::Age, [1_i64, 2])),
        "SELECT * FROM \"users\" WHERE \"age\" IN (?, ?)"
    );
}

#[test]
fn raw_fragment_rejects_bind_count_mismatch() {
    let missing = RawFragment::new("age > ? AND name = ?", vec![Value::I64(1)])
        .expect_err("two placeholders, one bind");
    assert_eq!((missing.placeholders(), missing.binds()), (2, 1));
    assert!(
        missing
            .to_string()
            .contains("2 `?` placeholders but 1 binds")
    );

    let extra = RawFragment::new("age > ?", vec![Value::I64(1), Value::I64(2)])
        .expect_err("one placeholder, two binds");
    assert_eq!((extra.placeholders(), extra.binds()), (1, 2));

    let sql = Query::<sqlx::Sqlite, User>::new()
        .raw_where(RawFragment::new("age > ?", vec![Value::I64(0)]).expect("counts match"))
        .sql();
    assert_eq!(sql, "SELECT * FROM \"users\" WHERE (age > ?)");
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
