//! A startup helper that serves a [`Router`] until the process ends, plus an
//! optional telemetry initializer.
//!
//! [`serve`] and [`serve_with_shutdown`] are thin wrappers over
//! [`axum::serve()`] that return its [`io::Result`] rather than panicking: the
//! caller decides how to surface a bind or accept failure, in keeping with the
//! library's no-panic rule. [`shutdown_signal`] resolves on the signals a
//! process is actually stopped with — `Ctrl-C`, and `SIGTERM` on Unix, which is
//! what a container runtime sends.
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use axum::{Router, routing::get};
//! use tokio::net::TcpListener;
//! use vivarium_web::serve::{serve_with_shutdown, shutdown_signal};
//!
//! let app = Router::new().route("/", get(|| async { "ok" }));
//! let listener = TcpListener::bind("0.0.0.0:8080").await?;
//! serve_with_shutdown(listener, app, shutdown_signal()).await
//! # }
//! ```
//!
//! Telemetry stays the application's decision — this crate never installs a
//! subscriber on its own. With the `telemetry` feature the
//! [`telemetry`] module offers the common setup: human-readable stdout lines
//! plus optional JSON logs rotated daily.

use std::future::Future;
use std::io;

use axum::Router;
use tokio::net::TcpListener;

/// Serve `app` on `listener` until the process shuts down.
///
/// This is a thin wrapper over [`axum::serve()`] that returns its [`io::Result`]
/// rather than panicking.
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

/// Serve `app` on `listener` until `shutdown` resolves, then drain in-flight
/// requests.
///
/// Pass [`shutdown_signal()`] for the usual process behaviour, or a channel
/// receiver when something else decides when to stop (a test, a supervisor, a
/// maintenance task).
///
/// # Example
///
/// ```no_run
/// # async fn example() -> std::io::Result<()> {
/// use axum::{Router, routing::get};
/// use tokio::net::TcpListener;
/// use vivarium_web::serve::serve_with_shutdown;
///
/// let app = Router::new().route("/", get(|| async { "ok" }));
/// let listener = TcpListener::bind("0.0.0.0:8080").await?;
///
/// let (stop, wait) = tokio::sync::oneshot::channel::<()>();
/// serve_with_shutdown(listener, app, async move {
///     let _ = wait.await;
///     // `stop` is dropped here, but a real program keeps it to signal later.
///     drop(stop);
/// })
/// .await
/// # }
/// ```
pub async fn serve_with_shutdown(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
}

/// Resolves when the process is asked to shut down.
///
/// Listens for `Ctrl-C` everywhere and additionally for `SIGTERM` on Unix (the
/// signal a container runtime or `systemd` sends). A failure to install either
/// handler is logged and then ignored — waiting forever is better than shutting
/// a healthy server down because a signal handler could not be registered.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "could not listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "could not listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(feature = "telemetry")]
pub mod telemetry {
    //! An optional tracing subscriber: stdout, plus JSON logs in a rotating
    //! file.
    //!
    //! [`init`] installs a process-wide subscriber: human-readable lines on
    //! stdout, and — when [`TelemetryOptions::json_file`] is set — one JSON line
    //! per event in a file rotated daily next to it. The returned
    //! [`TelemetryGuard`] owns the writer, so dropping it (at the end of `main`)
    //! flushes what is buffered: keep it alive, and do not `mem::forget` it.
    //!
    //! ```no_run
    //! # fn main() -> Result<(), vivarium_web::serve::telemetry::TelemetryError> {
    //! use vivarium_web::serve::telemetry::{TelemetryOptions, init};
    //!
    //! let _guard = init(TelemetryOptions {
    //!     level: Some("info,vivarium_web=debug".to_string()),
    //!     json_file: Some("/var/log/vivarium/app.json".into()),
    //!     ansi: false,
    //! })?;
    //! # Ok(())
    //! # }
    //! ```

    use std::path::PathBuf;

    use tracing_appender::non_blocking::WorkerGuard;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::prelude::*;

    /// How to initialize telemetry.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct TelemetryOptions {
        /// An `EnvFilter` directive string, such as `"info"` or
        /// `"vivarium_web=debug,info"`.
        ///
        /// `None` reads `RUST_LOG` and falls back to `info`.
        pub level: Option<String>,
        /// A JSON log file, rotated daily (the date is appended to the file
        /// name). `None` logs to stdout only.
        pub json_file: Option<PathBuf>,
        /// Whether stdout keeps ANSI colours. JSON lines never carry them.
        pub ansi: bool,
    }

    impl Default for TelemetryOptions {
        fn default() -> Self {
            Self {
                level: None,
                json_file: None,
                ansi: true,
            }
        }
    }

    /// Keeps the log pipeline alive; dropping it flushes and stops the writer.
    #[must_use = "the guard must stay alive, and drops to flush the log file"]
    pub struct TelemetryGuard {
        worker: Option<WorkerGuard>,
    }

    impl std::fmt::Debug for TelemetryGuard {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TelemetryGuard")
                .field("file", &self.worker.is_some())
                .finish()
        }
    }

    /// Why telemetry could not be initialized.
    #[derive(Debug, thiserror::Error)]
    pub enum TelemetryError {
        /// The filter directive could not be parsed.
        #[error("invalid log filter `{directive}`")]
        Filter {
            /// The rejected directive.
            directive: String,
            /// The parser's complaint.
            #[source]
            source: tracing_subscriber::filter::ParseError,
        },
        /// A global subscriber was already installed (by this call earlier, or
        /// by the application).
        #[error("a global tracing subscriber is already installed")]
        AlreadyInstalled,
    }

    /// Installs the subscriber described by `options`.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::Filter`] when `level` is not a valid filter
    /// directive, and [`TelemetryError::AlreadyInstalled`] when a global
    /// subscriber already exists — including a second call, since one process
    /// has one global subscriber.
    pub fn init(options: TelemetryOptions) -> Result<TelemetryGuard, TelemetryError> {
        let filter = match &options.level {
            Some(directive) => {
                EnvFilter::try_new(directive).map_err(|source| TelemetryError::Filter {
                    directive: directive.clone(),
                    source,
                })?
            }
            None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        };

        let stdout_layer = tracing_subscriber::fmt::layer().with_ansi(options.ansi);
        let (file, worker) = match &options.json_file {
            Some(path) => {
                let directory = match path.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                    _ => PathBuf::from("."),
                };
                let prefix = path.file_name().map_or_else(
                    || "vivarium".to_string(),
                    |name| name.to_string_lossy().into(),
                );
                let appender = tracing_appender::rolling::daily(directory, prefix);
                let (writer, guard) = tracing_appender::non_blocking(appender);
                (Some(writer), Some(guard))
            }
            None => (None, None),
        };

        let installed = match file {
            Some(writer) => tracing::subscriber::set_global_default(
                tracing_subscriber::registry()
                    .with(filter)
                    .with(stdout_layer)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_ansi(false)
                            .with_writer(writer),
                    ),
            ),
            None => tracing::subscriber::set_global_default(
                tracing_subscriber::registry()
                    .with(filter)
                    .with(stdout_layer),
            ),
        };
        installed.map_err(|_| TelemetryError::AlreadyInstalled)?;

        Ok(TelemetryGuard { worker })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn raw_get(addr: std::net::SocketAddr, path: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("write request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("read response");
        response
    }

    #[tokio::test]
    async fn serve_with_shutdown_answers_then_stops() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let app = Router::new().route("/", get(|| async { "ok" }));
        let (stop, wait) = tokio::sync::oneshot::channel::<()>();

        let server = tokio::spawn(serve_with_shutdown(listener, app, async move {
            let _ = wait.await;
        }));

        let response = raw_get(addr, "/").await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("ok"), "{response}");

        stop.send(()).expect("signal shutdown");
        server
            .await
            .expect("the server task joins")
            .expect("graceful shutdown is clean");
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_writes_json_to_a_file_and_flushes_on_drop() {
        use super::telemetry::{TelemetryError, TelemetryOptions, init};

        let directory = std::env::temp_dir().join(format!(
            "vivarium-telemetry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join("app.json");

        let guard = init(TelemetryOptions {
            level: Some("info".to_string()),
            json_file: Some(path),
            ansi: false,
        })
        .expect("installs the subscriber");

        tracing::info!(target: "vivarium_web::serve::tests", "telemetry probe");
        drop(guard);

        // The file name gets the date appended by the daily rotation.
        let written: Vec<_> = std::fs::read_dir(&directory)
            .expect("read the log directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        assert_eq!(written.len(), 1, "expected one rotated file: {written:?}");
        let contents = std::fs::read_to_string(&written[0]).expect("read the log");
        assert!(contents.contains("telemetry probe"), "{contents}");
        assert!(contents.contains("\"level\":\"INFO\""), "{contents}");

        // A second install cannot succeed: the process has one subscriber.
        assert!(matches!(
            init(TelemetryOptions::default()),
            Err(TelemetryError::AlreadyInstalled)
        ));

        std::fs::remove_dir_all(&directory).expect("clean up the temp directory");
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_rejects_an_invalid_filter() {
        use super::telemetry::{TelemetryError, TelemetryOptions, init};

        // A directive that cannot parse; no subscriber is installed before the
        // error, so this stays a pure validation test.
        let error = init(TelemetryOptions {
            level: Some("not a filter==".to_string()),
            json_file: None,
            ansi: false,
        })
        .expect_err("the directive must be rejected");
        assert!(matches!(error, TelemetryError::Filter { .. }), "{error:?}");
    }
}
