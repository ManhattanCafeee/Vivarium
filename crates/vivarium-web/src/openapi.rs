//! OpenAPI (utoipa) integration: schema-safe helpers, security schemes, spec
//! metadata and a one-call UI mount.
//!
//! # Conventions
//!
//! Downstream crates copy the handler template below. Four rules matter, and
//! the tests in `tests/openapi.rs` guard them:
//!
//! 1. the `Varser` family are opaque newtypes, so utoipa cannot see through
//!    them — write the inner type explicitly in `request_body = …` /
//!    `params(…)`;
//! 2. `operation_id` is required: it becomes the generated SDK's function name,
//!    and follows `<Tag>__<operation>` with a capitalized first letter
//!    (`tag = "user"`, `operation_id = "User__create"`);
//! 3. **error responses declare no body** — only `(status = …, description =
//!    "…")`. Declaring `body = ApiResponse<()>` would add an `ApiResponse_*`
//!    component for the unit type and change the generated error branch;
//! 4. an authenticated endpoint declares `security(("session" = []))` or
//!    `security(("bearer" = []))`, matching the scheme names registered by
//!    [`session_cookie_scheme`] and [`bearer_scheme`].
//!
//! ```rust,ignore
//! #[utoipa::path(
//!     post, path = "/users", tag = "user",
//!     operation_id = "User__create",
//!     request_body = CreateUserReq,
//!     responses(
//!         (status = 200, body = ApiResponse<UserResp>),
//!         (status = 400, description = "malformed or unparseable request"),
//!         (status = 409, description = "username or email already exists"),
//!     ),
//! )]
//! pub async fn create_user(
//!     State(state): State<AppState>,
//!     SessionCtx { user_id }: SessionCtx,
//!     Varser(req): Varser<CreateUserReq>,
//! ) -> Result<ApiResponse<UserResp>, ApiError> { /* … */ }
//! ```
//!
//! # Building the document
//!
//! ```
//! use utoipa::openapi::OpenApi;
//! use vivarium_web::openapi;
//!
//! /// Adds the crate's `info` block and the two security schemes.
//! fn configure(mut api: OpenApi) -> OpenApi {
//!     api.info = openapi::info("my api", "1.0.0", "What this API does.");
//!     let components = api.components.get_or_insert_with(Default::default);
//!     components.add_security_scheme("session", openapi::session_cookie_scheme("sid"));
//!     components.add_security_scheme("bearer", openapi::bearer_scheme());
//!     api
//! }
//! ```
//!
//! Collecting the paths and mounting the UIs (feature `utoipa-ui`) is the same
//! pattern with `OpenApiRouter` and `mount`.

#[cfg(feature = "utoipa-ui")]
use axum::Router;
#[cfg(feature = "utoipa-ui")]
use utoipa::openapi::OpenApi;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Info, InfoBuilder};

#[cfg(feature = "utoipa-ui")]
pub use utoipa_axum::router::OpenApiRouter;
#[cfg(feature = "utoipa-ui")]
pub use utoipa_axum::routes;

/// The session-cookie security scheme.
///
/// `cookie_name` must be the name the session middleware holds the id in, so
/// the generated clients send the right cookie.
pub fn session_cookie_scheme(cookie_name: &str) -> SecurityScheme {
    SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(cookie_name)))
}

/// The bearer-JWT security scheme (`type: http`, `scheme: bearer`,
/// `bearerFormat: JWT`).
pub fn bearer_scheme() -> SecurityScheme {
    SecurityScheme::Http(
        HttpBuilder::new()
            .scheme(HttpAuthScheme::Bearer)
            .bearer_format("JWT")
            .build(),
    )
}

/// The spec's `info` block, replacing utoipa's defaults (which describe
/// utoipa, not this API).
pub fn info(title: &str, version: &str, description: &str) -> Info {
    InfoBuilder::new()
        .title(title)
        .version(version)
        .description(Some(description))
        .build()
}

/// Mounts the API docs onto `router` under `base`.
///
/// Three routes are added:
///
/// - `{base}/openapi.json` — the document itself;
/// - `{base}/scalar` — the Scalar UI;
/// - `{base}/swagger-ui` — the Swagger UI.
///
/// `base` is normalized: `"api"`, `"/api"` and `"/api/"` all mount the same
/// paths, and `""` mounts them at the root.
///
/// ```
/// use vivarium_web::openapi;
/// use vivarium_web::openapi::{OpenApiRouter, routes};
/// use utoipa::openapi::OpenApi;
///
/// # #[utoipa::path(get, path = "/health", operation_id = "Health__get",
/// #     responses((status = 200, body = String)))]
/// # async fn health() -> String { "ok".to_string() }
/// # fn build(api: OpenApi) -> axum::Router {
/// let (router, api) = OpenApiRouter::new().routes(routes!(health)).split_for_parts();
/// let app = openapi::mount(router, "/api", api);
/// # app
/// # }
/// # let _ = build(OpenApi::default());
/// ```
#[cfg(feature = "utoipa-ui")]
pub fn mount(router: Router, base: &str, api: OpenApi) -> Router {
    use utoipa_scalar::{Scalar, Servable};
    use utoipa_swagger_ui::SwaggerUi;

    let base = if base.starts_with('/') {
        base.to_string()
    } else {
        format!("/{base}")
    };
    let base = base.trim_end_matches('/');

    let spec_url = format!("{base}/openapi.json");
    let scalar_url = format!("{base}/scalar");
    let swagger_url = format!("{base}/swagger-ui");

    // Scalar renders the document into its HTML, so it needs no route of its
    // own; Swagger UI serves the document from `spec_url` (which is why no
    // separate route is registered for it — that would collide).
    let scalar: Router = Scalar::with_url(scalar_url, api.clone()).into();
    let swagger: Router = SwaggerUi::new(swagger_url).url(spec_url, api).into();

    router.merge(scalar).merge(swagger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::openapi::Info;
    #[cfg(feature = "utoipa-ui")]
    use utoipa::openapi::OpenApiBuilder;

    /// The session scheme names the cookie the client must send.
    #[test]
    fn session_scheme_is_a_named_cookie() {
        let rendered = serde_json::to_value(session_cookie_scheme("sid")).expect("serializes");
        assert_eq!(
            rendered,
            serde_json::json!({ "type": "apiKey", "in": "cookie", "name": "sid" })
        );
    }

    /// The bearer scheme is HTTP bearer with a JWT format.
    #[test]
    fn bearer_scheme_is_http_bearer_jwt() {
        let rendered = serde_json::to_value(bearer_scheme()).expect("serializes");
        assert_eq!(
            rendered,
            serde_json::json!({ "type": "http", "scheme": "bearer", "bearerFormat": "JWT" })
        );
    }

    /// `info` replaces every default utoipa would otherwise emit.
    #[test]
    fn info_is_fully_specified() {
        let info = info("my api", "1.0.0", "does things");
        assert_eq!(info.title, "my api");
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.description.as_deref(), Some("does things"));
        assert!(info.license.is_none());
        assert!(info.contact.is_none());
        assert!(info != Info::default());
    }

    /// `mount` serves the document, both UIs, and normalizes `base`.
    #[cfg(feature = "utoipa-ui")]
    #[tokio::test]
    async fn mount_serves_the_document_and_both_uis() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let api = OpenApiBuilder::new()
            .info(info("mounted api", "0.3.0", "served by mount"))
            .build();

        // `base` is normalized: "api", "/api" and "/api/" mount the same paths.
        let app = mount(Router::new(), "/api/", api);
        let get = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();

        let response = app
            .clone()
            .oneshot(get("/api/openapi.json"))
            .await
            .expect("serves");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let spec: serde_json::Value = serde_json::from_slice(&bytes).expect("json spec");
        assert_eq!(spec["info"]["title"], "mounted api");

        let response = app
            .clone()
            .oneshot(get("/api/scalar"))
            .await
            .expect("serves");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert!(
            String::from_utf8_lossy(&bytes).contains("mounted api"),
            "the Scalar page embeds the document"
        );

        // Swagger UI redirects the bare path to its trailing-slash form.
        let response = app
            .clone()
            .oneshot(get("/api/swagger-ui"))
            .await
            .expect("serves");
        assert!(response.status().is_redirection(), "{}", response.status());
        let response = app
            .clone()
            .oneshot(get("/api/swagger-ui/"))
            .await
            .expect("serves");
        assert_eq!(response.status(), StatusCode::OK);

        let response = app.oneshot(get("/api/nope")).await.expect("serves");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// When `base` is empty the docs live at the server root.
    #[cfg(feature = "utoipa-ui")]
    #[tokio::test]
    async fn mount_without_a_base_serves_at_the_root() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let app = mount(Router::new(), "", OpenApiBuilder::new().build());
        let request = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();

        for uri in ["/openapi.json", "/scalar"] {
            let response = app.clone().oneshot(request(uri)).await.expect("serves");
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
        }
    }
}
