//! A startup helper that serves a [`Router`] until the process ends.

use std::io;

use axum::Router;
use tokio::net::TcpListener;

/// Serve `app` on `listener` until the process shuts down.
///
/// This is a thin wrapper over [`axum::serve()`] that returns its [`io::Result`]
/// rather than panicking: the caller decides how to surface a bind or accept
/// failure, in keeping with the library's no-panic rule.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> std::io::Result<()> {
/// use axum::{Router, routing::get};
/// use tokio::net::TcpListener;
/// use vivarium_web::serve::serve;
///
/// let app = Router::new().route("/", get(|| async { "ok" }));
/// let listener = TcpListener::bind("0.0.0.0:8080").await?;
/// serve(listener, app).await
/// # }
/// ```
pub async fn serve(listener: TcpListener, app: Router) -> io::Result<()> {
    axum::serve(listener, app).await
}
