//! The message catalog can be replaced once per process.
//!
//! Installation is process-wide, so this lives in its own integration test
//! binary: it installs a catalog and then asserts that every message the
//! library produces on its own behalf comes from it — including the echoed
//! detail of a custom `Deserialize` failure (the downstream case that parses
//! `message` for its own text).

#![cfg(feature = "validation-validator")]

use std::borrow::Cow;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;
use vivarium_web::texts::{Texts, install_texts, texts};
use vivarium_web::{ApiError, Initializer, PathVarser, SessionCtx, debug_mode, install_debug_mode};

/// A path id that rejects non-numeric values with a custom message.
#[derive(Debug)]
struct Id(String);

impl Id {
    /// The raw id, as received.
    fn value(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        if raw.chars().all(|character| character.is_ascii_digit()) {
            Ok(Id(raw))
        } else {
            Err(serde::de::Error::custom(format!("user id invalid: {raw}")))
        }
    }
}

#[derive(Debug, Deserialize, validator::Validate)]
struct IdPath {
    id: Id,
}

impl Initializer for IdPath {}

/// A numeric path id carrying a rule.
#[derive(Debug, Deserialize, validator::Validate)]
struct PosPath {
    #[validate(range(min = 1, message = "id must be positive"))]
    id: u32,
}

impl Initializer for PosPath {}

#[derive(Debug, Deserialize, validator::Validate)]
struct CreateReq {
    #[validate(length(min = 3, message = "too short"))]
    name: String,
}

impl Initializer for CreateReq {}

fn catalog() -> Texts {
    Texts {
        data_parse: Cow::Borrowed("payload unreadable"),
        bad_request: Cow::Borrowed("bad parameter"),
        validation: Cow::Borrowed("fields rejected"),
        unauthorized: Cow::Borrowed("no session"),
        internal: Cow::Borrowed("server exploded"),
        database: Cow::Borrowed("storage exploded"),
        echo_details: true,
        ..Texts::default()
    }
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

/// Installs the catalog and debug mode exactly once for the whole binary.
///
/// Installation is process-wide and tests run in parallel, so every test goes
/// through this instead of racing on the first call.
fn install_once() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        install_texts(catalog()).expect("the first install wins");
        install_debug_mode(true).expect("the first install wins");
    });
}

/// The first installation wins; a second one is reported as an error.
#[test]
fn installs_once_and_is_visible() {
    install_once();
    assert!(
        install_texts(Texts::default()).is_err(),
        "a second install must fail"
    );
    assert_eq!(texts().unauthorized, "no session");
    assert!(texts().echo_details);

    assert!(
        install_debug_mode(false).is_err(),
        "a second install must fail"
    );
    assert!(debug_mode());
}

/// `ApiError::internal` / `database` and validation read the installed text.
#[tokio::test]
async fn library_defaults_come_from_the_catalog() {
    install_once();
    let internal = ApiError::internal("boom");
    assert_eq!(internal.message(), "server exploded");
    assert_eq!(
        body_json(internal.into_response()).await["message"],
        "server exploded"
    );

    let database = ApiError::database("driver exploded");
    assert_eq!(database.message(), "storage exploded");

    let validation = ApiError::validation(vivarium_web::ValidationErrors::new());
    assert_eq!(validation.message(), "fields rejected");
}

/// A session-protected handler rejects with the catalog's `unauthorized` text.
#[tokio::test]
async fn session_rejection_uses_the_catalog() {
    install_once();
    let app = Router::new().route(
        "/me",
        get(|SessionCtx { user_id }: SessionCtx| async move { user_id.to_string() }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/me")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(response).await["message"], "no session");
}

/// `echo_details` repeats the upstream parse detail, so an application can keep
/// surfacing its own `Deserialize` message (the downstream contract).
#[tokio::test]
async fn echo_details_repeats_the_parse_detail() {
    install_once();
    let app = Router::new()
        .route(
            "/api/users/{id}",
            get(
                |PathVarser(param): PathVarser<IdPath>| async move { param.id.value().to_string() },
            ),
        )
        .route(
            "/api/items/{id}",
            get(|PathVarser(param): PathVarser<PosPath>| async move { param.id.to_string() }),
        );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/users/nope")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], 400);
    let message = body["message"].as_str().expect("message is a string");
    assert!(
        message.contains("user id invalid: nope"),
        "the custom Deserialize text must survive: {message}"
    );

    // A rule failure still uses the catalog text, with the detail in `errors`.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/items/0")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["message"], "fields rejected");
    assert_eq!(body["errors"]["id"][0]["message"], "id must be positive");
}

/// The body extractor's parse failure goes through the same detail pipeline.
#[tokio::test]
async fn body_parse_failure_echoes_the_detail() {
    install_once();
    use vivarium_web::Varser;

    let app = Router::new().route(
        "/users",
        axum::routing::post(|Varser(request): Varser<CreateReq>| async move { request.name }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/users")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .expect("request"),
        )
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], 400);
    // With `echo_details` the upstream serde text is repeated verbatim.
    let message = body["message"].as_str().expect("message is a string");
    assert!(
        message.contains("EOF while parsing"),
        "the serde detail must be echoed: {message}"
    );
}
