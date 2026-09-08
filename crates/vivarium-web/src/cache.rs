//! A tower [`Layer`] that stamps responses with a `Cache-Control` header.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

/// A tower layer that adds a `Cache-Control: public, max-age=<seconds>` header
/// to every response produced by the wrapped service.
#[derive(Clone, Copy, Debug)]
pub struct Cache {
    /// The number of seconds clients may cache responses for.
    pub seconds: u32,
}

/// The [`Service`] produced by [`Cache`]'s [`Layer`] implementation.
///
/// It clones the inner service and injects the `Cache-Control` header into each
/// response.
#[derive(Clone)]
pub struct CacheService<S> {
    inner: S,
    value: HeaderValue,
}

impl<S> Layer<S> for Cache {
    type Service = CacheService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        let value = cache_control_value(self.seconds);
        CacheService { inner, value }
    }
}

fn cache_control_value(seconds: u32) -> HeaderValue {
    HeaderValue::from_str(&format!("public, max-age={seconds}"))
        .unwrap_or_else(|_| HeaderValue::from_static("public"))
}

impl<S> Service<Request> for CacheService<S>
where
    S: Service<Request> + Clone + Send + 'static,
    S::Response: IntoResponse,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = CacheServiceFuture<S>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let mut inner = self.inner.clone();
        let value = self.value.clone();
        CacheServiceFuture {
            inner: inner.call(req),
            value,
        }
    }
}

/// Future returned by [`CacheService`].
///
/// Polls the inner service and injects the `Cache-Control` header into the
/// resulting response.
pub struct CacheServiceFuture<S>
where
    S: Service<Request>,
{
    inner: S::Future,
    value: HeaderValue,
}

impl<S> Future for CacheServiceFuture<S>
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
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, this.value.clone());
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
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
    use axum::routing::get;
    use tower::ServiceExt;

    #[tokio::test]
    async fn cache_adds_header() {
        let app = Router::new()
            .route("/", get(|| async { "hello" }))
            .layer(Cache { seconds: 300 });

        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let response = app.oneshot(req).await.expect("request succeeds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("public, max-age=300"))
        );
    }
}
