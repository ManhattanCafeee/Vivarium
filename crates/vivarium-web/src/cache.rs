//! A tower [`Layer`] that stamps responses with a `Cache-Control` header.
//!
//! [`CacheControl`] states the policy and [`CacheControl::layer`] applies it.
//! Two rules keep the layer from making decisions that are not its own:
//!
//! - A response that already carries `Cache-Control` keeps it. A handler that
//!   knows better than the route's default is never overruled.
//! - Only successful and redirect responses (2xx and 3xx) are stamped. A 401
//!   from an authentication layer, a 404 from a lookup or a 500 from a database
//!   failure must not be cacheable, so those pass through untouched.
//!
//! ```no_run
//! use axum::{Router, routing::get};
//! use vivarium_web::cache::CacheControl;
//!
//! let app: Router = Router::new()
//!     .route("/articles", get(|| async { "…" }))
//!     .layer(CacheControl::Public(300).layer());
//! ```

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

/// What a set of responses may be cached as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheControl {
    /// `public, max-age=<seconds>`: any cache — including a shared one — may
    /// store the response.
    Public(u32),
    /// `private, max-age=<seconds>`: only the client's own cache may store it.
    Private(u32),
    /// `no-cache`: a cache may store it, but must revalidate before every use.
    NoCache,
    /// `no-store`: never written to a cache.
    NoStore,
}

impl CacheControl {
    /// A tower layer applying this policy to the responses of a route.
    pub fn layer(self) -> CacheControlLayer {
        CacheControlLayer { control: self }
    }

    /// The `Cache-Control` value this policy renders.
    fn header_value(self) -> HeaderValue {
        match self {
            Self::Public(seconds) => HeaderValue::from_str(&format!("public, max-age={seconds}")),
            Self::Private(seconds) => HeaderValue::from_str(&format!("private, max-age={seconds}")),
            Self::NoCache => Ok(HeaderValue::from_static("no-cache")),
            Self::NoStore => Ok(HeaderValue::from_static("no-store")),
        }
        // `from_str` cannot fail on the literal ASCII above.
        .unwrap_or_else(|_| HeaderValue::from_static("no-store"))
    }
}

/// The [`Layer`] returned by [`CacheControl::layer`].
#[derive(Clone, Copy, Debug)]
pub struct CacheControlLayer {
    control: CacheControl,
}

impl<S> Layer<S> for CacheControlLayer {
    type Service = CacheControlService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CacheControlService {
            inner,
            value: self.control.header_value(),
        }
    }
}

/// The [`Service`] produced by [`CacheControlLayer`].
///
/// It clones the inner service and stamps the `Cache-Control` header onto the
/// responses it is allowed to touch.
#[derive(Clone)]
pub struct CacheControlService<S> {
    inner: S,
    value: HeaderValue,
}

impl<S> Service<Request> for CacheControlService<S>
where
    S: Service<Request> + Clone + Send + 'static,
    S::Response: IntoResponse,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = CacheControlFuture<S>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let mut inner = self.inner.clone();
        let value = self.value.clone();
        CacheControlFuture {
            inner: inner.call(req),
            value,
        }
    }
}

/// Future returned by [`CacheControlService`].
///
/// Polls the inner service and stamps the `Cache-Control` header onto a
/// response that neither carries one already nor failed.
pub struct CacheControlFuture<S>
where
    S: Service<Request>,
{
    inner: S::Future,
    value: HeaderValue,
}

impl<S> Future for CacheControlFuture<S>
where
    S: Service<Request>,
    S::Response: IntoResponse,
{
    type Output = Result<Response, S::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `inner` is never moved once this future is pinned; we only
        // project the pin through to the inner future before polling it.
        let this = unsafe { self.get_unchecked_mut() };
        let inner = unsafe { Pin::new_unchecked(&mut this.inner) };
        match inner.poll(cx) {
            Poll::Ready(Ok(response)) => {
                let mut response = response.into_response();
                let status = response.status();
                if !response.headers().contains_key(header::CACHE_CONTROL)
                    && (status.is_success() || status.is_redirection())
                {
                    response
                        .headers_mut()
                        .insert(header::CACHE_CONTROL, this.value.clone());
                }
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Redirect;
    use axum::routing::get;
    use tower::ServiceExt;

    use crate::error::ApiError;

    async fn header_of(app: Router, uri: &str) -> (StatusCode, Option<String>) {
        let request = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("request builds");
        let response = app.oneshot(request).await.expect("request succeeds");
        let status = response.status();
        let header = response
            .headers()
            .get(header::CACHE_CONTROL)
            .map(|value| value.to_str().unwrap_or("").to_string());
        (status, header)
    }

    fn app_with(control: CacheControl, status: StatusCode) -> Router {
        let handler = move || async move { (status, "body") };
        Router::new()
            .route("/x", get(handler))
            .layer(control.layer())
    }

    #[tokio::test]
    async fn policies_render_the_documented_header() {
        for (control, expected) in [
            (CacheControl::Public(60), "public, max-age=60"),
            (CacheControl::Private(60), "private, max-age=60"),
            (CacheControl::NoCache, "no-cache"),
            (CacheControl::NoStore, "no-store"),
        ] {
            let (status, header) = header_of(app_with(control, StatusCode::OK), "/x").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(header.as_deref(), Some(expected), "policy {control:?}");
        }
    }

    #[tokio::test]
    async fn redirects_are_stamped_too() {
        let app = Router::new()
            .route("/old", get(|| async { Redirect::temporary("/new") }))
            .layer(CacheControl::Public(60).layer());

        let (status, header) = header_of(app, "/old").await;
        assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(header.as_deref(), Some("public, max-age=60"));
    }

    #[tokio::test]
    async fn failures_are_left_alone() {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::NOT_FOUND,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let (actual, header) =
                header_of(app_with(CacheControl::Public(60), status), "/x").await;
            assert_eq!(actual, status);
            assert_eq!(header, None, "{status} must not be cacheable");
        }

        // The same holds for an error the library renders itself.
        let app = Router::new()
            .route("/boom", get(|| async { ApiError::internal("boom") }))
            .layer(CacheControl::Public(60).layer());
        let (status, header) = header_of(app, "/boom").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(header, None);
    }

    #[tokio::test]
    async fn an_existing_header_wins() {
        let app = Router::new()
            .route(
                "/x",
                get(|| async { ([(header::CACHE_CONTROL, "private, max-age=1")], "body") }),
            )
            .layer(CacheControl::Public(600).layer());

        let (status, header) = header_of(app, "/x").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(header.as_deref(), Some("private, max-age=1"));
    }
}
