//! The generated OpenAPI document follows this crate's conventions.
//!
//! Four minimal handlers stand in for a real API — a success, a nested page
//! envelope, a validation failure and an authentication failure. The test
//! renders their spec and checks the parts a generated SDK depends on: every
//! operation has an `operation_id`, `ApiResponse<…>` becomes an
//! `ApiResponse_*` component named `{Base}_{Child}`, both security schemes are
//! registered, error responses declare **no** body, `info` comes from
//! [`vivarium_web::openapi::info`], `additionalProperties` is never the empty
//! schema, and schema descriptions stay prose (no doctest code blocks).
//!
//! The whole document is compared against `tests/golden/openapi.json`; run with
//! `UPDATE_GOLDEN=1` to regenerate it.

#![cfg(all(
    feature = "utoipa",
    feature = "utoipa-ui",
    feature = "validation-validator"
))]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use utoipa::ToSchema;
use vivarium_core::Page;
use vivarium_web::openapi::{self, OpenApiRouter, routes};
use vivarium_web::{ApiError, ApiResponse, Initializer, SessionCtx, Varser};

/// The title the golden document is generated with.
const TITLE: &str = "vivarium-web golden";

#[derive(Debug, Serialize, ToSchema)]
struct User {
    id: i64,
    name: String,
}

#[derive(Debug, Deserialize, ToSchema, validator::Validate)]
struct CreateUserReq {
    #[validate(length(min = 3, message = "too short"))]
    name: String,
}

impl Initializer for CreateUserReq {}

/// The success case: a list wrapped in the response envelope.
#[utoipa::path(
    get, path = "/users", tag = "user",
    operation_id = "User__list",
    responses(
        (status = 200, body = ApiResponse<Vec<User>>),
        (status = 401, description = "authentication required"),
    ),
    security(("session" = [])),
)]
async fn list_users() -> Result<axum::Json<ApiResponse<Vec<User>>>, ApiError> {
    Ok(axum::Json(ApiResponse::ok(Vec::new())))
}

/// The nested-envelope case: a page of users. `Page<User>` pins utoipa's
/// composed component name for a generic child (`Page`'s own name is fixed by
/// its derive in `vivarium-core`).
#[utoipa::path(
    get, path = "/users/page", tag = "user",
    operation_id = "User__page",
    responses(
        (status = 200, body = ApiResponse<Page<User>>),
        (status = 401, description = "authentication required"),
    ),
    security(("session" = [])),
)]
async fn page_users() -> Result<axum::Json<ApiResponse<Page<User>>>, ApiError> {
    Ok(axum::Json(ApiResponse::ok(Page {
        items: Vec::new(),
        total: 0,
        page: 1,
        per_page: 20,
    })))
}

/// The validation-failure case: a body extractor that can reject with 422.
#[utoipa::path(
    post, path = "/users", tag = "user",
    operation_id = "User__create",
    request_body = CreateUserReq,
    responses(
        (status = 200, body = ApiResponse<User>),
        // Error responses declare no body: see the crate's OpenAPI conventions.
        (status = 400, description = "malformed or unparseable request"),
        (status = 422, description = "validation failed"),
    ),
)]
async fn create_user(
    Varser(request): Varser<CreateUserReq>,
) -> Result<axum::Json<ApiResponse<User>>, ApiError> {
    Ok(axum::Json(ApiResponse::ok(User {
        id: 1,
        name: request.name,
    })))
}

/// The authentication-failure case: a session-protected endpoint.
#[utoipa::path(
    get, path = "/users/me", tag = "user",
    operation_id = "User__me",
    responses(
        (status = 200, body = ApiResponse<User>),
        (status = 401, description = "authentication required"),
    ),
    security(("bearer" = [])),
)]
async fn me(_session: SessionCtx) -> Result<axum::Json<ApiResponse<User>>, ApiError> {
    Err(ApiError::not_found("no such user"))
}

/// Builds the document the way a downstream crate would.
fn build() -> (Router, utoipa::openapi::OpenApi) {
    let (router, mut api) = OpenApiRouter::new()
        .routes(routes!(list_users))
        .routes(routes!(page_users))
        .routes(routes!(create_user))
        .routes(routes!(me))
        .split_for_parts();

    api.info = openapi::info(TITLE, "0.3.0", "The golden OpenAPI document.");
    let components = api.components.get_or_insert_with(Default::default);
    components.add_security_scheme("session", openapi::session_cookie_scheme("sid"));
    components.add_security_scheme("bearer", openapi::bearer_scheme());

    (router, api)
}

/// The methods OpenAPI describes; anything else in a path item is metadata.
const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

fn spec_json() -> serde_json::Value {
    let (_, api) = build();
    serde_json::to_value(&api).expect("the document serializes")
}

/// Every operation is addressable by a generated SDK function name.
#[test]
fn every_operation_has_an_operation_id() {
    let spec = spec_json();
    let mut operations = 0;
    for (path, item) in spec["paths"].as_object().expect("paths is an object") {
        for (method, operation) in item.as_object().expect("path item is an object") {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            operations += 1;
            let operation_id = operation["operationId"]
                .as_str()
                .unwrap_or_else(|| panic!("{method} {path} has no operation_id"));

            // `<Tag>__<operation>` with a capitalized first letter and a
            // lower-case tag, as the conventions require.
            assert!(
                operation_id.contains("__"),
                "operation_id must be `<Tag>__<operation>`: {operation_id}"
            );
            assert!(
                operation_id.chars().next().is_some_and(char::is_uppercase),
                "operation_id must start with an upper-case letter: {operation_id}"
            );
            let tag = operation["tags"][0]
                .as_str()
                .unwrap_or_else(|| panic!("{operation_id} has no tag"));
            assert_eq!(tag, tag.to_lowercase(), "tags are lower-case: {tag}");
        }
    }
    assert_eq!(operations, 4, "the four golden handlers");
}

/// `ApiResponse<T>` components keep utoipa's `{Base}_{Child}` naming.
#[test]
fn api_response_schemas_follow_the_naming_rule() {
    let spec = spec_json();
    let schemas = spec["components"]["schemas"]
        .as_object()
        .expect("schemas is an object");

    let mut envelopes = schemas
        .keys()
        .filter(|name| name.starts_with("ApiResponse"))
        .cloned()
        .collect::<Vec<_>>();
    envelopes.sort();

    assert_eq!(
        envelopes,
        vec![
            "ApiResponse_Page_User".to_string(),
            "ApiResponse_User".to_string(),
            "ApiResponse_Vec_User".to_string()
        ],
        "the envelope components are exactly the instantiated ones"
    );

    for name in &envelopes {
        for segment in name.split('_').skip(1) {
            assert!(
                segment.chars().next().is_some_and(char::is_uppercase),
                "`{name}` must be named `{{Base}}_{{Child}}`"
            );
        }
    }
}

/// Both security schemes are registered with the shapes the middleware uses.
#[test]
fn both_security_schemes_are_registered() {
    let spec = spec_json();
    let schemes = &spec["components"]["securitySchemes"];

    assert_eq!(
        schemes["session"],
        serde_json::json!({ "type": "apiKey", "in": "cookie", "name": "sid" })
    );
    assert_eq!(
        schemes["bearer"],
        serde_json::json!({ "type": "http", "scheme": "bearer", "bearerFormat": "JWT" })
    );

    // And the endpoints reference them.
    assert_eq!(
        spec["paths"]["/users"]["get"]["security"],
        serde_json::json!([{ "session": [] }])
    );
    assert_eq!(
        spec["paths"]["/users/me"]["get"]["security"],
        serde_json::json!([{ "bearer": [] }])
    );
}

/// Error responses describe a status and a reason, never a body: declaring one
/// would add an `ApiResponse_TupleUnit` component and change the SDK's error
/// branch.
#[test]
fn error_responses_have_no_body() {
    let spec = spec_json();
    let mut checked = 0;
    for (path, item) in spec["paths"].as_object().expect("paths is an object") {
        for (method, operation) in item.as_object().expect("path item is an object") {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            for (status, response) in operation["responses"]
                .as_object()
                .expect("responses is an object")
            {
                assert!(
                    response["description"].is_string(),
                    "{method} {path} {status} needs a description"
                );
                if status.starts_with('4') || status.starts_with('5') {
                    checked += 1;
                    assert!(
                        response.get("content").is_none(),
                        "{method} {path} {status} must not declare a body: {response}"
                    );
                }
            }
        }
    }
    assert!(checked >= 4, "the golden handlers declare error responses");
    assert!(
        spec["components"]["schemas"]
            .get("ApiResponse_TupleUnit")
            .is_none(),
        "no unit envelope leaked into the components"
    );
}

/// `info` describes this API, not utoipa.
#[test]
fn info_comes_from_the_helper() {
    let spec = spec_json();
    assert_eq!(spec["info"]["title"], TITLE);
    assert_eq!(spec["info"]["version"], "0.3.0");
    assert_eq!(spec["info"]["description"], "The golden OpenAPI document.");
    assert_ne!(spec["info"]["title"], "utoipa-axum");
}

/// Every `description` string in `value`, at any nesting depth.
fn collect_descriptions(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if key == "description"
                    && let Some(text) = value.as_str()
                {
                    out.push(text.to_owned());
                }
                collect_descriptions(value, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_descriptions(item, out);
            }
        }
        _ => {}
    }
}

/// Every `additionalProperties` value in `value`, at any nesting depth.
fn collect_additional_properties<'a>(
    value: &'a serde_json::Value,
    out: &mut Vec<&'a serde_json::Value>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if key == "additionalProperties" {
                    out.push(value);
                }
                collect_additional_properties(value, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_additional_properties(item, out);
            }
        }
        _ => {}
    }
}

/// `ToSchema` derives a type's whole doc comment into its schema
/// `description`, so a doctest written above a schema type leaks into
/// generated SDK docs.
#[test]
fn schema_descriptions_contain_no_doctests() {
    let spec = spec_json();
    let mut descriptions = Vec::new();
    collect_descriptions(&spec["components"], &mut descriptions);

    assert!(
        descriptions.len() >= 8,
        "the golden schemas carry descriptions"
    );
    for description in descriptions {
        assert!(
            !description.contains("```"),
            "a doc-comment code block leaked into a schema description: {description}"
        );
    }
}

/// An `additionalProperties` schema is never the empty object: utoipa emits
/// `{}` for a map whose value type carries no schema of its own
/// (`serde_json::Value`), and the explicit boolean is the form this crate
/// ships. The document's one free-form map must also still *be* free-form —
/// an empty schema is not the only wrong answer, a closed object is another.
#[test]
fn no_schema_has_an_empty_additional_properties() {
    let spec = spec_json();
    let mut values = Vec::new();
    collect_additional_properties(&spec, &mut values);

    assert!(
        values.len() >= 2,
        "the golden schemas declare additionalProperties"
    );
    for value in values {
        assert!(
            !matches!(value, serde_json::Value::Object(fields) if fields.is_empty()),
            "`additionalProperties: {{}}` is an empty schema: {value}"
        );
    }

    // `FieldViolation::params` carries the failed rule's parameters, which have
    // no schema to point at: `"additionalProperties": false` would forbid the
    // very keys the server sends.
    assert_eq!(
        spec["components"]["schemas"]["FieldViolation"]["properties"]["params"]["additionalProperties"],
        serde_json::json!(true)
    );
}

/// `localize` hands every schema description to the closure and replaces only
/// what it matches — the composed envelopes, the `$ref`s utoipa puts inside
/// `oneOf`, and `vivarium-core`'s `Page` included.
#[test]
fn localize_replaces_the_texts_the_hook_matches() {
    let (_, mut api) = build();
    let before = serde_json::to_value(&api).expect("the document serializes");

    let replaced = openapi::localize(&mut api, |text| match text {
        "The unified response envelope." => Some(std::borrow::Cow::Borrowed("envelope")),
        "One page of results." => Some(std::borrow::Cow::Borrowed("page of results")),
        "The rows of this page." => Some(std::borrow::Cow::Borrowed("rows")),
        "The validation violations; present only for a failed validation." => {
            Some(std::borrow::Cow::Borrowed("violations"))
        }
        _ => None,
    });
    assert_eq!(
        replaced, 8,
        "three envelopes, two page texts, three validation refs"
    );

    let after = serde_json::to_value(&api).expect("the document serializes");
    let schemas = &after["components"]["schemas"];
    for name in [
        "ApiResponse_User",
        "ApiResponse_Vec_User",
        "ApiResponse_Page_User",
    ] {
        assert_eq!(schemas[name]["description"], "envelope", "{name}");
        assert_eq!(
            schemas[name]["properties"]["errors"]["oneOf"][1]["description"], "violations",
            "{name}"
        );
    }
    assert_eq!(
        schemas["ApiResponse_Page_User"]["properties"]["data"]["description"],
        "page of results"
    );
    assert_eq!(
        schemas["ApiResponse_Page_User"]["properties"]["data"]["properties"]["items"]["description"],
        "rows",
        "a description on an array schema is reached too"
    );
    assert_eq!(
        schemas["FieldViolation"]["description"],
        before["components"]["schemas"]["FieldViolation"]["description"],
        "an unmatched description is left alone"
    );

    // A hook that matches nothing changes nothing.
    let (_, mut untouched) = build();
    assert_eq!(openapi::localize(&mut untouched, |_| None), 0);
    assert_eq!(
        serde_json::to_value(&untouched).expect("the document serializes"),
        before
    );

    // `Some` counts even when the replacement is the text itself.
    let (_, mut echoed) = build();
    let echoed_count = openapi::localize(&mut echoed, |text| {
        (text == "One page of results.")
            .then_some(std::borrow::Cow::Borrowed("One page of results."))
    });
    assert_eq!(echoed_count, 1);
}

/// The golden file is the whole document; regenerate it with `UPDATE_GOLDEN=1`.
#[test]
fn document_matches_the_golden_file() {
    let (_, api) = build();
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&api).expect("the document serializes")
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/openapi.json");

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &rendered).expect("write the golden file");
        return;
    }

    let golden = std::fs::read_to_string(&path).expect("the golden file exists");
    assert_eq!(
        rendered, golden,
        "the spec drifted from tests/golden/openapi.json; regenerate it with UPDATE_GOLDEN=1"
    );
}

/// The spec's promises match what the handlers really do over HTTP.
#[tokio::test]
async fn the_golden_handlers_serve_what_they_declare() {
    let (router, _) = build();

    let request = |method: &str, uri: &str, body: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let body_json = async |response: axum::response::Response| {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("json body")
    };

    // Success.
    let response = router
        .clone()
        .oneshot(request("GET", "/users", ""))
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        serde_json::json!({ "code": 0, "message": "ok", "data": [] })
    );

    // The nested page envelope.
    let response = router
        .clone()
        .oneshot(request("GET", "/users/page", ""))
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": { "items": [], "total": 0, "page": 1, "per_page": 20 }
        })
    );

    // A validation failure is the 422 the spec declares, with structured errors.
    let response = router
        .clone()
        .oneshot(request("POST", "/users", r#"{"name":"ab"}"#))
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["errors"]["name"][0]["code"], "length");

    // An unparseable body is the 400 the spec declares.
    let response = router
        .clone()
        .oneshot(request("POST", "/users", "{"))
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // An unauthenticated call is the 401 the spec declares.
    let response = router
        .oneshot(request("GET", "/users/me", ""))
        .await
        .expect("serves");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
