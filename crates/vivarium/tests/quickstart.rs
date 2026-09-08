//! End-to-end acceptance: the README quick-start service — SQLite + axum +
//! Varser + CRUD — must actually run. Mirrors the plan's Phase 5 criterion:
//! from `cargo add vivarium`, the example works.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Router, routing::get, routing::post};
use garde::Validate;
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;
use vivarium::sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use vivarium::{ApiError, Initializer, Varser, create};

#[derive(Clone, sqlx::FromRow, vivarium::Entity)]
#[entity(table = "users")]
struct User {
    id: i64,
    name: String,
}

#[derive(Deserialize, Validate)]
struct NewUser {
    #[garde(length(chars, min = 1, max = 100))]
    name: String,
}

impl Initializer for NewUser {}

async fn create_user(
    state: axum::extract::State<SqlitePool>,
    Varser(new_user): Varser<NewUser>,
) -> Result<(), ApiError> {
    create(
        &state.0,
        User {
            id: 0,
            name: new_user.name,
        },
    )
    .await
    .map_err(|e| ApiError::Internal {
        system: e.to_string(),
    })?;
    Ok(())
}

async fn count_users(state: axum::extract::State<SqlitePool>) -> Result<String, ApiError> {
    let n = vivarium::count::<User, _>(&state.0)
        .await
        .map_err(|e| ApiError::Internal {
            system: e.to_string(),
        })?;
    Ok(n.to_string())
}

async fn app() -> (Router, SqlitePool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    vivarium::MIGRATOR.run(&pool).await.expect("migrate");
    let router = Router::new()
        .route("/users", post(create_user))
        .route("/users/count", get(count_users))
        .with_state(pool.clone());
    (router, pool)
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
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(bytes, "1");

    let rows: i64 = vivarium::sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn validation_failure_returns_422() {
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
}
