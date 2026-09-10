//! End-to-end acceptance: the README quick-start service — SQLite + axum +
//! `Varser` + the `ApiResponse` envelope + CRUD — must actually run. Mirrors
//! the plan's acceptance criterion: from `cargo add vivarium-rs`, the example
//! works.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Router, routing::get, routing::post};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use validator::Validate;
use vivarium_rs::sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium_rs::{ApiError, ApiResponse, Initializer, Varser, create};

#[derive(Clone, sqlx::FromRow, vivarium_rs::Entity)]
#[entity(table = "users")]
struct User {
    id: i64,
    name: String,
}

#[derive(Deserialize, Validate)]
struct NewUser {
    #[validate(length(min = 1, max = 100, message = "name must be 1-100 characters"))]
    name: String,
}

impl Initializer for NewUser {}

#[derive(Serialize)]
struct UserJson {
    id: i64,
    name: String,
}

async fn create_user(
    state: axum::extract::State<SqlitePool>,
    Varser(new_user): Varser<NewUser>,
) -> Result<ApiResponse<UserJson>, ApiError> {
    let id = create(
        &state.0,
        User {
            id: 0,
            name: new_user.name.clone(),
        },
    )
    .await
    .map_err(ApiError::database)?;
    Ok(ApiResponse::ok(UserJson {
        id,
        name: new_user.name,
    }))
}

async fn count_users(
    state: axum::extract::State<SqlitePool>,
) -> Result<ApiResponse<i64>, ApiError> {
    let count = vivarium_rs::count::<User, _>(&state.0)
        .await
        .map_err(ApiError::database)?;
    Ok(ApiResponse::ok(count))
}

async fn app() -> (Router, SqlitePool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    sqlx::migrate!("./tests/migrations")
        .run(&pool)
        .await
        .expect("migrate");
    let router = Router::new()
        .route("/users", post(create_user))
        .route("/users/count", get(count_users))
        .with_state(pool.clone());
    (router, pool)
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn quickstart_service_runs() {
    let (router, pool) = app().await;

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/users")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"ada"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": { "id": 1, "name": "ada" },
        })
    );

    let response = router
        .oneshot(
            Request::builder()
                .uri("/users/count")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        serde_json::json!({ "code": 0, "message": "ok", "data": 1 })
    );

    let rows: i64 = vivarium_rs::sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn validation_failure_returns_422_with_structured_errors() {
    let (router, _pool) = app().await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/users")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":""}"#))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["code"], 422);
    let violation = &body["errors"]["name"][0];
    assert_eq!(violation["code"], "length");
    assert_eq!(violation["message"], "name must be 1-100 characters");
    assert_eq!(violation["params"]["min"], 1);
    assert_eq!(violation["params"]["max"], 100);
    assert_eq!(body["data"], serde_json::Value::Null);
}

#[tokio::test]
async fn malformed_body_returns_400_data_parse() {
    let (router, _pool) = app().await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/users")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": 42}"#))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], 400);
    assert_eq!(body["message"], "invalid request data");
    assert_eq!(body["data"], serde_json::Value::Null);
}
