//! # vivarium-config
//!
//! Typed, hot-reloadable configuration for the [`vivarium-rs`] family.
//! Built on [`figment`] (TOML/YAML/JSON), [`notify`] (file watching), and
//! [`arc-swap`](https://docs.rs/arc-swap) (lock-free reads).
//!
//! Reads via [`Config::get`] are lock-free; reloads atomically swap in a new
//! value and notify registered handlers in registration order.
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs

#![deny(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use notify::Watcher;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;

/// A handler invoked on every successful reload with a reference to the new
/// configuration value.
type UpdateHandler<T> = Arc<dyn Fn(&T) + Send + Sync>;

/// Errors produced while loading, reloading, or watching a [`Config`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file could not be parsed or extracted by [`figment`].
    #[error("failed to parse configuration: {0}")]
    Figment(Box<figment::Error>),

    /// An I/O error occurred while accessing the config file.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A file-watching error occurred.
    #[error("file watch error: {0}")]
    Notify(#[from] notify::Error),

    /// The config file's extension is not a supported format.
    #[error("unsupported config file extension `{0}` (expected toml, yaml, yml, or json)")]
    UnsupportedExtension(String),
}

impl From<figment::Error> for ConfigError {
    fn from(err: figment::Error) -> Self {
        ConfigError::Figment(Box::new(err))
    }
}

/// A typed configuration value that can be reloaded from disk and watched for
/// changes.
///
/// `T` is the type the configuration deserializes into. The current value is
/// read lock-free via [`Config::get`]; reloads atomically swap in a new value
/// and notify handlers in registration order.
pub struct Config<T> {
    inner: ArcSwap<T>,
    path: PathBuf,
    handlers: RwLock<Vec<(usize, UpdateHandler<T>)>>,
    next_id: AtomicUsize,
}

impl<T> Config<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    /// Loads configuration from the file at `path`.
    ///
    /// The file format is selected by extension: `.toml`, `.yml`, `.yaml`, or
    /// `.json`. The path is canonicalized and stored for later
    /// [`Config::reload`] and [`Config::watch`] calls.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] if the path cannot be canonicalized, the file
    /// cannot be read or parsed, extraction into `T` fails, or the extension is
    /// unsupported.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let canonical = std::fs::canonicalize(path.as_ref())?;
        let config: T = load_from_path(&canonical)?;

        Ok(Self {
            inner: ArcSwap::from_pointee(config),
            path: canonical,
            handlers: RwLock::new(Vec::new()),
            next_id: AtomicUsize::new(0),
        })
    }

    /// Returns the current configuration value.
    ///
    /// The returned [`Arc`] is a lock-free snapshot: it remains valid even if
    /// the configuration is subsequently reloaded.
    pub fn get(&self) -> Arc<T> {
        self.inner.load_full()
    }

    /// Registers `handler` to be invoked on every successful reload, in
    /// registration order.
    ///
    /// Returns a stable index identifying the handler. Registering the same
    /// closure more than once is allowed; each registration receives a distinct
    /// index. Handlers accumulate for the lifetime of the [`Config`].
    pub fn register<F>(&self, handler: F) -> usize
    where
        F: Fn(&T) + Send + Sync + 'static,
    {
        let index = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.handlers.write().push((index, Arc::new(handler)));
        index
    }

    /// Reloads configuration from the stored path.
    ///
    /// On success the new value is atomically swapped in and every registered
    /// handler is called exactly once, in registration order, each receiving a
    /// reference to the new value. The handler list is snapshotted under a read
    /// lock and invoked after the lock is released, so handlers may safely call
    /// [`Config::register`] or [`Config::reload`] without deadlocking.
    ///
    /// On failure the previous value is kept and no handler is called.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] if the file cannot be read, parsed, or
    /// extracted into `T`.
    pub fn reload(&self) -> Result<Arc<T>, ConfigError> {
        let new_config: T = load_from_path(&self.path)?;
        let new_arc = Arc::new(new_config);

        self.inner.store(Arc::clone(&new_arc));

        let snapshot: Vec<UpdateHandler<T>> = {
            let guard = self.handlers.read();
            guard
                .iter()
                .map(|(_, handler)| Arc::clone(handler))
                .collect()
        };

        for handler in &snapshot {
            handler(&new_arc);
        }

        Ok(new_arc)
    }

    /// Watches the config file for changes and reloads automatically.
    ///
    /// Installs a polling [`notify::PollWatcher`] on the config file's parent
    /// directory (so atomic-save renames are observed), filters events for the
    /// config file, debounces bursts with a short drain window, and calls
    /// [`Config::reload`] on each change. Reload failures are logged to stderr
    /// and watching continues. The watcher is kept alive by a spawned thread
    /// that owns the event channel; it runs until program exit.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] if the watcher cannot be created, the parent
    /// directory cannot be watched, or the watcher thread cannot be spawned.
    pub fn watch(self: Arc<Self>) -> Result<(), ConfigError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| {
                ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "config file has no parent directory",
                ))
            })?
            .to_path_buf();

        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let poll_config = notify::Config::default().with_poll_interval(Duration::from_millis(100));

        let mut watcher = notify::PollWatcher::new(tx, poll_config)?;
        watcher.watch(&parent, notify::RecursiveMode::NonRecursive)?;

        let target = self.path.clone();

        std::thread::Builder::new()
            .name("vivarium-config-watcher".to_owned())
            .spawn(move || {
                // Keep the watcher (and its internal poll loop) alive for the
                // lifetime of this thread.
                let _watcher = watcher;
                loop {
                    match rx.recv() {
                        Ok(Ok(event)) => {
                            if event.paths.iter().any(|p| p == &target) {
                                drain_debounce(&rx);
                                if let Err(err) = self.reload() {
                                    eprintln!("vivarium-config: reload failed: {err}");
                                }
                            }
                        }
                        Ok(Err(err)) => {
                            eprintln!("vivarium-config: watch error: {err}");
                        }
                        Err(_) => return,
                    }
                }
            })?;

        Ok(())
    }
}

/// Parses and extracts a `T` from `path`, selecting the provider by extension.
fn load_from_path<T>(path: &Path) -> Result<T, ConfigError>
where
    T: DeserializeOwned,
{
    use figment::providers::{Format, Json, Toml, Yaml};

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);

    let figment = match extension.as_deref() {
        Some("toml") => figment::Figment::from(Toml::file(path)),
        Some("yml") | Some("yaml") => figment::Figment::from(Yaml::file(path)),
        Some("json") => figment::Figment::from(Json::file(path)),
        other => {
            return Err(ConfigError::UnsupportedExtension(
                other.map_or_else(|| "<none>".to_owned(), str::to_owned),
            ));
        }
    };

    Ok(figment.extract()?)
}

/// Drains queued events for a short debounce window.
fn drain_debounce(rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>) {
    let deadline = Instant::now() + Duration::from_millis(100);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Deserialize)]
    struct AppConfig {
        name: String,
        port: u16,
    }

    fn toml(name: &str, port: u16) -> String {
        format!("name = \"{name}\"\nport = {port}\n")
    }

    #[test]
    fn load_reads_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml("hello", 8080)).unwrap();

        let config: Config<AppConfig> = Config::load(&path).unwrap();
        let value = config.get();
        assert_eq!(value.name, "hello");
        assert_eq!(value.port, 8080);
    }

    #[test]
    fn register_returns_increasing_indices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml("old", 1)).unwrap();
        let config: Config<AppConfig> = Config::load(&path).unwrap();

        let a = config.register(|_: &AppConfig| {});
        let b = config.register(|_: &AppConfig| {});
        // Duplicate registration of the same closure is allowed (distinct index).
        let c = config.register(|_: &AppConfig| {});

        assert_eq!((a, b, c), (0, 1, 2));
    }

    #[test]
    fn reload_calls_handlers_once_in_order_with_new_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml("old", 1)).unwrap();
        let config: Config<AppConfig> = Config::load(&path).unwrap();

        let calls: Arc<Mutex<Vec<(usize, String)>>> = Arc::new(Mutex::new(Vec::new()));

        let c0 = Arc::clone(&calls);
        config.register(move |c: &AppConfig| {
            c0.lock().push((0, c.name.clone()));
        });

        let c1 = Arc::clone(&calls);
        config.register(move |c: &AppConfig| {
            c1.lock().push((1, c.name.clone()));
        });

        std::fs::write(&path, toml("new", 2)).unwrap();
        let new_arc = config.reload().unwrap();
        assert_eq!(new_arc.name, "new");

        let recorded = calls.lock();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0], (0, "new".to_owned()));
        assert_eq!(recorded[1], (1, "new".to_owned()));
    }

    #[test]
    fn reload_failure_keeps_old_config_and_calls_no_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml("old", 1)).unwrap();
        let config: Config<AppConfig> = Config::load(&path).unwrap();

        let calls: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        config.register(move |_: &AppConfig| {
            *counter.lock() += 1;
        });

        std::fs::write(&path, "not valid toml [[[").unwrap();
        let err = config.reload().unwrap_err();
        assert!(matches!(err, ConfigError::Figment(_)));

        // Old value retained, no handler invoked.
        assert_eq!(config.get().name, "old");
        assert_eq!(*calls.lock(), 0);
    }

    #[test]
    fn unsupported_extension_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.ini");
        std::fs::write(&path, "name = \"hello\"\n").unwrap();

        let err = Config::<AppConfig>::load(&path)
            .err()
            .expect("expected load to fail for unsupported extension");
        assert!(matches!(err, ConfigError::UnsupportedExtension(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watch_reloads_on_file_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml("old", 1)).unwrap();

        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());
        let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let notify = Arc::new(tokio::sync::Notify::new());

        let obs = Arc::clone(&observed);
        let n = Arc::clone(&notify);
        config.register(move |c: &AppConfig| {
            *obs.lock() = Some(c.name.clone());
            n.notify_one();
        });

        let handle = Arc::clone(&config);
        config.watch().unwrap();

        // Cross the poll watcher's 1-second mtime granularity before rewriting.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        std::fs::write(&path, toml("new", 2)).unwrap();

        tokio::time::timeout(Duration::from_secs(10), notify.notified())
            .await
            .expect("timed out waiting for watch-triggered reload");

        assert_eq!(*observed.lock(), Some("new".to_owned()));
        assert_eq!(handle.get().name, "new");
    }
}
