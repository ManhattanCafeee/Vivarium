//! # vivarium-config
//!
//! Typed, hot-reloadable configuration for the [`vivarium-rs`] family.
//!
//! Values are collected from an ordered stack of [figment] sources — code-level
//! defaults, a config file, environment variables, and arbitrary user-supplied
//! providers — read lock-free through [`Config::get`], reloaded on demand with
//! [`Config::reload`], and reloaded automatically on file changes with
//! [`Config::watch`].
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use serde::{Deserialize, Serialize};
//! use vivarium_config::{Config, ConfigOptions};
//!
//! #[derive(Debug, Deserialize, Serialize)]
//! struct AppConfig {
//!     port: u16,
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Each source overrides the previous one:
//! // defaults < config.toml < APP__* environment variables < extra providers.
//! let config = Arc::new(Config::load_with(
//!     ConfigOptions::new("config.toml")
//!         .defaults(&AppConfig { port: 8080 })
//!         .file()
//!         .env_prefixed("APP")
//!         .separator("__"),
//! )?);
//!
//! config.register(|cfg: &AppConfig| println!("port is now {}", cfg.port));
//! config.on_error(|err| eprintln!("config error: {err}"));
//!
//! let watcher = Arc::clone(&config).watch()?;
//! let cfg = config.get(); // lock-free snapshot
//! println!("port is {}", cfg.port);
//! drop(watcher); // stops watching
//! # Ok(())
//! # }
//! ```
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs

#![deny(missing_docs)]

use std::any::Any;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use figment::providers::{Env, Format, Json, Serialized, Toml, Yaml};
use figment::value::{Dict, Map as ProfileMap};
use figment::{Figment, Metadata, Profile, Provider};
use notify::{RecursiveMode, Watcher};
use parking_lot::RwLock;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// How long a burst of file events is drained before a single reload runs.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Name given to the background watcher thread.
const WATCHER_THREAD: &str = "vivarium-config-watcher";

/// A handler invoked with the new value after every notifying reload.
type UpdateHandler<T> = Arc<dyn Fn(&T) + Send + Sync>;

/// A handler invoked with every configuration error.
type ErrorHandler = Arc<dyn Fn(&ConfigError) + Send + Sync>;

/// Identifies a handler registered with [`Config::register`] or
/// [`Config::on_error`].
///
/// Ids are unique within a [`Config`] and are accepted by
/// [`Config::unregister`] for both update and error handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandlerId(u64);

impl fmt::Display for HandlerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "handler#{}", self.0)
    }
}

/// Errors produced while loading, reloading, or watching a [`Config`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// [`Config::load_with`] was given options that configure no source at all.
    #[error(
        "no configuration source was configured; add at least one of defaults, file, env, or merge"
    )]
    NoSource,

    /// A file source (or a watcher) was requested without a configured path.
    #[error("no configuration file path was configured (use `ConfigOptions::new(path)`)")]
    MissingPath,

    /// The configuration could not be parsed or extracted by [`figment`].
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

    /// An update handler or a user-supplied [`Provider`] panicked; the panic
    /// was caught and reported here.
    ///
    /// Watching continues after this error.
    #[error("configuration handler or provider panicked: {0}")]
    HandlerPanic(String),

    /// The watcher thread panicked, so it could not be joined cleanly.
    #[error("the configuration watcher thread panicked")]
    WatcherThreadPanicked,
}

impl From<figment::Error> for ConfigError {
    fn from(err: figment::Error) -> Self {
        ConfigError::Figment(Box::new(err))
    }
}

/// Builder describing the ordered sources a [`Config`] is loaded from.
///
/// Sources are merged in a fixed order, each one overriding the values of the
/// ones before it:
///
/// 1. [`defaults`](ConfigOptions::defaults) — defaults compiled into the
///    application;
/// 2. [`file`](ConfigOptions::file) — a `.toml`, `.yaml`, `.yml`, or `.json`
///    file selected by extension;
/// 3. [`env_prefixed`](ConfigOptions::env_prefixed) — environment variables
///    (with [`separator`](ConfigOptions::separator) nesting);
/// 4. [`merge`](ConfigOptions::merge) — user-supplied [`figment::Provider`]s,
///    in call order, so the last `merge` call wins over everything else.
///
/// At least one source must be configured; [`Config::load_with`] otherwise
/// returns [`ConfigError::NoSource`].
///
/// ```
/// use vivarium_config::ConfigOptions;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Deserialize, Serialize)]
/// struct AppConfig { port: u16 }
///
/// let options = ConfigOptions::new("config.toml")
///     .defaults(&AppConfig { port: 8080 })
///     .file()
///     .env_prefixed("APP")
///     .separator("__");
/// # let _ = options;
/// ```
#[derive(Clone, Default)]
pub struct ConfigOptions {
    path: Option<PathBuf>,
    file: bool,
    file_required: bool,
    defaults: Option<SharedProvider>,
    env_prefix: Option<String>,
    separator: Option<String>,
    merges: Vec<SharedProvider>,
}

impl fmt::Debug for ConfigOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigOptions")
            .field("path", &self.path)
            .field("file", &self.file)
            .field("file_required", &self.file_required)
            .field("defaults", &self.defaults.is_some())
            .field("env_prefix", &self.env_prefix)
            .field("separator", &self.separator)
            .field("merges", &self.merges.len())
            .finish()
    }
}

impl ConfigOptions {
    /// Creates options for the config file at `path`.
    ///
    /// The path alone configures no source: call [`ConfigOptions::file`] (or
    /// [`ConfigOptions::file_required`]) to read it, or add other sources.
    /// The path is still used to watch the file with [`Config::watch`].
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: Some(path.as_ref().to_path_buf()),
            ..Self::default()
        }
    }

    /// Adds `value` as the lowest-priority source, serialized with `serde`.
    ///
    /// The value is serialized immediately, so it is captured as it was at the
    /// time of this call and needs no `Clone` bound.
    pub fn defaults<V>(mut self, value: &V) -> Self
    where
        V: Serialize + ?Sized,
    {
        self.defaults = Some(SharedProvider::new(FixedProvider::serialize(value)));
        self
    }

    /// Adds the config file at [`ConfigOptions::new`]'s path as a source.
    ///
    /// The file format is selected by extension: `.toml`, `.yml`, `.yaml`, or
    /// `.json`. A missing file is not an error: the source simply contributes
    /// nothing, which lets an application run on defaults plus environment
    /// before the file exists.
    pub fn file(mut self) -> Self {
        self.file = true;
        self.file_required = false;
        self
    }

    /// Adds the config file at [`ConfigOptions::new`]'s path as a required
    /// source.
    ///
    /// Identical to [`ConfigOptions::file`] except that a missing file is
    /// reported as [`ConfigError::Io`]. This is what [`Config::load`] uses.
    pub fn file_required(mut self) -> Self {
        self.file = true;
        self.file_required = true;
        self
    }

    /// Adds environment variables starting with `prefix` as a source.
    ///
    /// The prefix is stripped from every matching variable name. With
    /// [`separator`](ConfigOptions::separator) set, the separator is stripped
    /// too and the remaining name is split on it to build nested keys, so
    /// `APP` + `__` reads `APP__AUTH__JWT__SECRET` as `auth.jwt.secret`.
    pub fn env_prefixed<P: Into<String>>(mut self, prefix: P) -> Self {
        self.env_prefix = Some(prefix.into());
        self
    }

    /// Sets the separator used to nest environment variables.
    ///
    /// Has an effect only together with [`env_prefixed`](ConfigOptions::env_prefixed),
    /// and applies to the part of the variable name after the prefix.
    pub fn separator<S: Into<String>>(mut self, separator: S) -> Self {
        self.separator = Some(separator.into());
        self
    }

    /// Adds an arbitrary [`figment::Provider`] above every other source.
    ///
    /// Providers are applied in call order, so a later `merge` overrides an
    /// earlier one. This is the extension point for application-specific
    /// sources such as legacy flat environment variables or a remote config
    /// service. The provider is kept and re-read on every reload, so it may
    /// return different data each time.
    pub fn merge<P>(mut self, provider: P) -> Self
    where
        P: Provider + Send + Sync + 'static,
    {
        self.merges.push(SharedProvider::new(provider));
        self
    }

    /// Returns `true` when no source is configured.
    fn is_empty(&self) -> bool {
        self.defaults.is_none() && !self.file && self.env_prefix.is_none() && self.merges.is_empty()
    }

    /// Resolves the config file path, canonicalizing it when it exists.
    ///
    /// Returns `None` when no path was configured. A required file that cannot
    /// be canonicalized (because it does not exist) is an [`ConfigError::Io`];
    /// an optional one keeps its location relative to a canonicalized parent
    /// directory so it can be watched before it is created.
    fn resolve_path(&self) -> Result<Option<PathBuf>, ConfigError> {
        let Some(path) = self.path.as_deref() else {
            return if self.file {
                Err(ConfigError::MissingPath)
            } else {
                Ok(None)
            };
        };

        match std::fs::canonicalize(path) {
            Ok(canonical) => Ok(Some(canonical)),
            Err(err) if self.file_required => Err(ConfigError::Io(err)),
            Err(_) => Ok(Some(absolutize(path))),
        }
    }

    /// Builds the source stack, using `path` for the file source.
    ///
    /// The stack is rebuilt on every load and reload, so environment variables
    /// and user providers are re-read each time.
    fn figment(&self, path: Option<&Path>) -> Result<Figment, ConfigError> {
        if self.is_empty() {
            return Err(ConfigError::NoSource);
        }

        let mut figment = Figment::new();

        if let Some(defaults) = &self.defaults {
            figment = figment.merge(defaults.clone());
        }

        if self.file {
            let path = path.ok_or(ConfigError::MissingPath)?;
            if self.file_required && !path.exists() {
                // `Toml::file`/`Json::file`/`Yaml::file` treat a missing file
                // as an empty source, so without this check a deleted required
                // file would silently fall back to the other layers.
                return Err(ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("required config file is missing: {}", path.display()),
                )));
            }
            figment = merge_file(figment, path)?;
        }

        if let Some(prefix) = &self.env_prefix {
            figment = figment.merge(env_provider(prefix, self.separator.as_deref()));
        }

        for provider in &self.merges {
            figment = figment.merge(provider.clone());
        }

        Ok(figment)
    }
}

/// A typed configuration value that can be reloaded and watched for changes.
///
/// `T` is deserialized from the source stack built by [`ConfigOptions`]. It
/// must also implement [`Serialize`], because reloads compare the serialized
/// form of the new value with the current one to skip no-op reloads.
///
/// The current value is read lock-free via [`Config::get`]. A reload atomically
/// swaps in the new value and then calls the update handlers in registration
/// order, on the thread that triggered the reload — for watcher-driven reloads
/// that is the watcher thread, so a slow handler delays reloads.
pub struct Config<T> {
    value: ArcSwap<T>,
    options: ConfigOptions,
    path: Option<PathBuf>,
    handlers: RwLock<Vec<(HandlerId, UpdateHandler<T>)>>,
    error_handlers: RwLock<Vec<(HandlerId, ErrorHandler)>>,
    next_id: AtomicU64,
    always_notify: AtomicBool,
    poll_interval: RwLock<Option<Duration>>,
}

impl<T> Config<T>
where
    T: DeserializeOwned + Serialize + Send + Sync + 'static,
{
    /// Loads configuration from the file at `path`.
    ///
    /// The file format is selected by extension: `.toml`, `.yml`, `.yaml`, or
    /// `.json`. The path is canonicalized and stored for later
    /// [`Config::reload`] and [`Config::watch`] calls.
    ///
    /// Use [`Config::load_with`] to layer defaults, environment variables, or
    /// additional providers around the file.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] if the path cannot be canonicalized — which
    /// includes a missing file — the file cannot be read or parsed, extraction
    /// into `T` fails, or the extension is unsupported.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        Self::load_with(ConfigOptions::new(path).file_required())
    }

    /// Loads configuration from the sources described by `options`.
    ///
    /// Sources are merged with later ones overriding earlier ones, see
    /// [`ConfigOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::NoSource`] if `options` configure no source,
    /// [`ConfigError::MissingPath`] if a file source has no path,
    /// [`ConfigError::Io`] if a required file does not exist, and
    /// [`ConfigError::Figment`] if the merged sources cannot be extracted
    /// into `T`.
    pub fn load_with(options: ConfigOptions) -> Result<Self, ConfigError> {
        let path = options.resolve_path()?;
        let value: T = options.figment(path.as_deref())?.extract()?;

        Ok(Self {
            value: ArcSwap::from_pointee(value),
            options,
            path,
            handlers: RwLock::new(Vec::new()),
            error_handlers: RwLock::new(Vec::new()),
            next_id: AtomicU64::new(0),
            always_notify: AtomicBool::new(false),
            poll_interval: RwLock::new(None),
        })
    }

    /// Returns the current configuration value.
    ///
    /// The returned [`Arc`] is a lock-free snapshot: it remains valid even if
    /// the configuration is subsequently reloaded.
    pub fn get(&self) -> Arc<T> {
        self.value.load_full()
    }

    /// Registers `handler` to be invoked on every notifying reload, in
    /// registration order.
    ///
    /// Handlers receive the new value, run on the thread that triggered the
    /// reload, and are called after the current value has been swapped in.
    /// Registering the same closure more than once is allowed; each
    /// registration receives a distinct [`HandlerId`]. A handler that panics
    /// is reported through [`Config::on_error`] and does not prevent the
    /// remaining handlers or later reloads from running.
    pub fn register<F>(&self, handler: F) -> HandlerId
    where
        F: Fn(&T) + Send + Sync + 'static,
    {
        let id = self.next_handler_id();
        self.handlers.write().push((id, Arc::new(handler)));
        id
    }

    /// Registers `handler` to be invoked with every configuration error, in
    /// registration order.
    ///
    /// Errors that reach this handler are reload failures, file-watching
    /// errors, and panics of other handlers or of user providers. A panicking
    /// error handler is logged and ignored.
    pub fn on_error<F>(&self, handler: F) -> HandlerId
    where
        F: Fn(&ConfigError) + Send + Sync + 'static,
    {
        let id = self.next_handler_id();
        self.error_handlers.write().push((id, Arc::new(handler)));
        id
    }

    /// Removes the handler registered under `id`.
    ///
    /// Returns `true` if an update handler or an error handler was removed,
    /// `false` if the id is unknown or was already unregistered. A reload that
    /// is already running may still call the handler once, because it works on
    /// a snapshot taken before the removal.
    pub fn unregister(&self, id: HandlerId) -> bool {
        let mut removed = remove_handler(&mut self.handlers.write(), id);
        removed |= remove_handler(&mut self.error_handlers.write(), id);
        removed
    }

    /// Reloads the configuration from the configured sources.
    ///
    /// The source stack is re-evaluated from scratch. On success the new value
    /// is atomically swapped in and, if it differs from the previous value,
    /// every registered handler is called exactly once, in registration order,
    /// each receiving a reference to the new value. The handler list is
    /// snapshotted under a read lock and invoked after the lock is released,
    /// so handlers may safely call [`Config::register`], [`Config::unregister`],
    /// or [`Config::reload`] without deadlocking.
    ///
    /// A reload whose value serializes identically to the current value is a
    /// no-op for handlers unless [`Config::always_notify`] was called; the
    /// value is still swapped in.
    ///
    /// On failure the previous value is kept, no update handler is called, and
    /// the error is reported through [`Config::on_error`] and `tracing::warn!`
    /// before it is returned.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] if the sources cannot be read, parsed, or
    /// extracted into `T`.
    pub fn reload(&self) -> Result<Arc<T>, ConfigError> {
        let value: T = match self.extract() {
            Ok(value) => value,
            Err(err) => {
                self.report_error(&err);
                return Err(err);
            }
        };

        let new_value = Arc::new(value);
        let notify = self.always_notify.load(Ordering::Relaxed) || self.differs(&new_value);
        self.value.store(Arc::clone(&new_value));

        if notify {
            self.dispatch(&new_value);
        }

        Ok(new_value)
    }

    /// Extracts a fresh value from the source stack.
    fn extract(&self) -> Result<T, ConfigError> {
        Ok(self.options.figment(self.path.as_deref())?.extract()?)
    }

    /// Calls every update handler with `value`, catching panics.
    fn dispatch(&self, value: &T) {
        let handlers: Vec<UpdateHandler<T>> = {
            let guard = self.handlers.read();
            guard.iter().map(|(_, h)| Arc::clone(h)).collect()
        };

        for handler in handlers {
            let outcome = catch_unwind(AssertUnwindSafe(|| handler(value)));
            if let Err(payload) = outcome {
                let panic = ConfigError::HandlerPanic(panic_message(&*payload));
                self.report_error(&panic);
            }
        }
    }

    /// Reports `err` to every error handler, after logging it.
    fn report_error(&self, err: &ConfigError) {
        tracing::warn!(error = %err, "vivarium-config: configuration error");

        let handlers: Vec<ErrorHandler> = {
            let guard = self.error_handlers.read();
            guard.iter().map(|(_, h)| Arc::clone(h)).collect()
        };

        for handler in handlers {
            let outcome = catch_unwind(AssertUnwindSafe(|| handler(err)));
            if outcome.is_err() {
                tracing::warn!("vivarium-config: an error handler panicked");
            }
        }
    }

    /// Returns `true` when `new_value` differs from the current value.
    ///
    /// Values that cannot be serialized are treated as different, so a
    /// comparison failure never swallows a notification.
    fn differs(&self, new_value: &T) -> bool {
        let current = self.value.load();
        match (fingerprint(new_value), fingerprint(&**current)) {
            (Ok(new), Ok(current)) => new != current,
            _ => true,
        }
    }

    /// Watches the config file and reloads on changes.
    ///
    /// The watcher observes the config file's parent directory
    /// non-recursively — so atomic-save renames are caught — and ignores
    /// events for other files. Bursts of events for the config file are
    /// debounced by draining them for a short window before a single reload
    /// runs.
    ///
    /// # Behaviour
    ///
    /// - **No initial reload**: the value read right after `watch()` is the
    ///   one produced by [`Config::load`] or [`Config::load_with`]. The first
    ///   reload happens on the first observed change.
    /// - By default the native [`notify::RecommendedWatcher`] is used;
    ///   [`Config::poll_interval`] opts into polling instead.
    /// - Reload failures and watcher errors are reported through
    ///   [`Config::on_error`] and `tracing::warn!`; watching then continues.
    /// - Values identical to the current one do not call update handlers
    ///   unless [`Config::always_notify`] was called.
    /// - Handlers run on the watcher thread.
    ///
    /// The returned [`ConfigWatcher`] stops and joins the watcher thread when
    /// it is dropped, so it must be kept alive for as long as watching is
    /// wanted.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] if no config file path was configured, if the
    /// watcher backend cannot be created, if the parent directory cannot be
    /// watched, or if the watcher thread cannot be spawned.
    pub fn watch(self: Arc<Self>) -> Result<ConfigWatcher, ConfigError> {
        let path = self.path.clone().ok_or(ConfigError::MissingPath)?;
        let parent = path
            .parent()
            .ok_or_else(|| {
                ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "config file has no parent directory",
                ))
            })?
            .to_path_buf();

        let (tx, rx) = mpsc::channel::<WatcherSignal>();
        let stop = tx.clone();
        let events = {
            let tx = tx.clone();
            move |event: notify::Result<notify::Event>| {
                let _ = tx.send(WatcherSignal::Event(event));
            }
        };
        drop(tx);

        let mut watcher: Box<dyn Watcher + Send> = match *self.poll_interval.read() {
            Some(interval) => Box::new(notify::PollWatcher::new(
                events,
                notify::Config::default().with_poll_interval(interval),
            )?),
            None => Box::new(notify::RecommendedWatcher::new(
                events,
                notify::Config::default(),
            )?),
        };
        watcher.watch(&parent, RecursiveMode::NonRecursive)?;

        let config = Arc::clone(&self);
        let thread = thread::Builder::new()
            .name(WATCHER_THREAD.to_owned())
            .spawn(move || {
                // Keep the watcher (and its backend thread) alive for as long
                // as this thread runs; dropping it stops event delivery.
                let _watcher = watcher;

                loop {
                    match rx.recv() {
                        Ok(WatcherSignal::Event(Ok(event))) => {
                            if !event.paths.iter().any(|observed| observed == &path) {
                                continue;
                            }
                            if drain_events(&rx) {
                                return;
                            }
                            // A panic escaping `reload`, such as a user
                            // provider panicking in `data()`, must not kill
                            // this thread or updates would stop silently.
                            match catch_unwind(AssertUnwindSafe(|| config.reload())) {
                                Ok(Ok(_)) => {}
                                // `reload` reports extraction failures itself.
                                Ok(Err(_)) => {}
                                Err(payload) => {
                                    let panic = ConfigError::HandlerPanic(panic_message(&*payload));
                                    config.report_error(&panic);
                                }
                            }
                        }
                        Ok(WatcherSignal::Event(Err(err))) => {
                            config.report_error(&ConfigError::Notify(err));
                        }
                        Ok(WatcherSignal::Stop) | Err(_) => return,
                    }
                }
            })?;

        Ok(ConfigWatcher {
            stop: Some(stop),
            thread_id: thread.thread().id(),
            thread: Some(thread),
        })
    }

    /// Makes every successful reload notify update handlers, even when the
    /// reloaded value is identical to the current one.
    ///
    /// By default — and to avoid pointless work on tooling that rewrites files
    /// with unchanged content — a reload that produces the same serialized
    /// value does not call update handlers.
    pub fn always_notify(&self) -> &Self {
        self.always_notify.store(true, Ordering::Relaxed);
        self
    }

    /// Uses a polling watcher with the given interval instead of the native
    /// watcher.
    ///
    /// Polling trades latency and CPU for portability, and is the fallback for
    /// filesystems that do not deliver reliable native events, such as network
    /// mounts. The default is [`notify::RecommendedWatcher`].
    ///
    /// The interval is read when [`Config::watch`] creates the watcher, so set
    /// it before watching. A polling watcher can miss a change that stays
    /// inside one modification-time bucket of the watched file.
    pub fn poll_interval(&self, interval: Duration) -> &Self {
        *self.poll_interval.write() = Some(interval);
        self
    }

    /// Allocates the next handler id.
    fn next_handler_id(&self) -> HandlerId {
        HandlerId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }
}

/// Handle to a running watcher started by [`Config::watch`].
///
/// Dropping the handle stops watching: a stop signal is sent to the watcher
/// thread and the thread is joined, so no thread is leaked. Use
/// [`ConfigWatcher::stop`] to observe a failing join.
#[must_use = "dropping the watcher stops watching the configuration file"]
pub struct ConfigWatcher {
    stop: Option<Sender<WatcherSignal>>,
    thread: Option<JoinHandle<()>>,
    thread_id: ThreadId,
}

impl fmt::Debug for ConfigWatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigWatcher")
            .field("thread", &self.thread_id)
            .finish_non_exhaustive()
    }
}

impl ConfigWatcher {
    /// Stops watching and waits for the watcher thread to finish.
    ///
    /// Equivalent to dropping the handle, except that a join failure is
    /// returned instead of being logged.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::WatcherThreadPanicked`] if the watcher thread
    /// panicked.
    pub fn stop(mut self) -> Result<(), ConfigError> {
        self.shutdown()
    }

    /// Signals and joins the watcher thread.
    fn shutdown(&mut self) -> Result<(), ConfigError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(WatcherSignal::Stop);
        }

        let Some(thread) = self.thread.take() else {
            return Ok(());
        };

        if thread.thread().id() == thread::current().id() {
            // Dropped from inside a handler running on the watcher thread:
            // joining would deadlock, and the signalled thread exits on its
            // own as soon as the handler returns.
            return Ok(());
        }

        thread
            .join()
            .map_err(|_| ConfigError::WatcherThreadPanicked)
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        if let Err(err) = self.shutdown() {
            tracing::warn!(error = %err, "vivarium-config: stopping the watcher failed");
        }
    }
}

/// A signal delivered to the watcher thread.
enum WatcherSignal {
    /// A file-system event from the notify backend.
    Event(notify::Result<notify::Event>),
    /// A request to stop watching and exit the thread.
    Stop,
}

/// A provider held behind an [`Arc`] so the source stack can be replayed on
/// every reload.
#[derive(Clone)]
struct SharedProvider(Arc<dyn Provider + Send + Sync>);

impl SharedProvider {
    /// Wraps `provider` for repeated use.
    fn new<P>(provider: P) -> Self
    where
        P: Provider + Send + Sync + 'static,
    {
        Self(Arc::new(provider))
    }
}

impl Provider for SharedProvider {
    fn metadata(&self) -> Metadata {
        self.0.metadata()
    }

    fn data(&self) -> Result<ProfileMap<Profile, Dict>, figment::Error> {
        self.0.data()
    }

    /// Forwarded because `Figment::merge` reads it: dropping it would silently
    /// extract under `Profile::Default` instead of the provider's profile.
    fn profile(&self) -> Option<Profile> {
        self.0.profile()
    }
}

/// A provider that always yields the same dictionary captured at build time.
#[derive(Clone)]
struct FixedProvider {
    metadata: Metadata,
    data: Result<ProfileMap<Profile, Dict>, Box<figment::Error>>,
}

impl FixedProvider {
    /// Captures `value` as the dictionary figment would emit for it.
    fn serialize<V>(value: &V) -> Self
    where
        V: Serialize + ?Sized,
    {
        Self {
            metadata: Serialized::defaults(value).metadata(),
            data: fingerprint(value),
        }
    }
}

impl Provider for FixedProvider {
    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn data(&self) -> Result<ProfileMap<Profile, Dict>, figment::Error> {
        self.data.clone().map_err(|err| *err)
    }
}

/// Serializes `value` into the dictionary form figment uses for it.
///
/// Two values with equal dictionaries produce the same configuration, which is
/// what reload de-duplication compares.
fn fingerprint<T>(value: &T) -> Result<ProfileMap<Profile, Dict>, Box<figment::Error>>
where
    T: Serialize + ?Sized,
{
    Serialized::defaults(value).data().map_err(Box::new)
}

/// Adds the file at `path` to `figment`, selecting the provider by extension.
fn merge_file(figment: Figment, path: &Path) -> Result<Figment, ConfigError> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);

    let figment = match extension.as_deref() {
        Some("toml") => figment.merge(Toml::file(path)),
        Some("yml" | "yaml") => figment.merge(Yaml::file(path)),
        Some("json") => figment.merge(Json::file(path)),
        other => {
            return Err(ConfigError::UnsupportedExtension(
                other.map_or_else(|| "<none>".to_owned(), str::to_owned),
            ));
        }
    };

    Ok(figment)
}

/// Builds the environment variable provider for `prefix` and `separator`.
///
/// With a separator, both the prefix and the separator are stripped from the
/// variable name, so `APP` + `__` turns `APP__AUTH__JWT` into `auth.jwt`.
fn env_provider(prefix: &str, separator: Option<&str>) -> Env {
    match separator {
        Some(separator) => Env::prefixed(&format!("{prefix}{separator}")).split(separator),
        None => Env::prefixed(prefix),
    }
}

/// Drains queued events for a short debounce window.
///
/// Returns `true` when the watcher was asked to stop, either by a stop signal
/// or by the event channel closing.
fn drain_events(rx: &Receiver<WatcherSignal>) -> bool {
    let deadline = Instant::now() + DEBOUNCE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }

        match rx.recv_timeout(remaining) {
            Ok(WatcherSignal::Event(_)) => {}
            Ok(WatcherSignal::Stop) | Err(RecvTimeoutError::Disconnected) => return true,
            Err(RecvTimeoutError::Timeout) => return false,
        }
    }
}

/// Returns `path` with its parent directory canonicalized when possible.
fn absolutize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };

    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
            Ok(parent) => parent.join(name),
            Err(_) => absolute,
        },
        _ => absolute,
    }
}

/// Extracts a human-readable message from a panic payload.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

/// Removes the handler with `id` from `handlers`.
///
/// Returns `true` if a handler was removed.
fn remove_handler<T>(handlers: &mut Vec<(HandlerId, T)>, id: HandlerId) -> bool {
    let Some(position) = handlers.iter().position(|(handler, _)| *handler == id) else {
        return false;
    };

    handlers.remove(position);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde::Deserialize;
    use std::sync::atomic::AtomicUsize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct AppConfig {
        name: String,
        port: u16,
    }

    fn toml(name: &str, port: u16) -> String {
        format!("name = \"{name}\"\nport = {port}\n")
    }

    /// Creates a temp directory holding a `config.toml` with `contents`.
    ///
    /// The directory must be kept alive for as long as the file is used.
    fn config_file(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).expect("write config");
        (dir, path)
    }

    #[test]
    fn load_reads_fields() {
        let (_dir, path) = config_file(&toml("hello", 8080));

        let config: Config<AppConfig> = Config::load(&path).unwrap();
        let value = config.get();
        assert_eq!(value.name, "hello");
        assert_eq!(value.port, 8080);
    }

    #[test]
    fn load_rejects_missing_and_unsupported_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.toml");

        let err = Config::<AppConfig>::load(&missing)
            .err()
            .expect("missing file");
        assert!(matches!(err, ConfigError::Io(_)), "{err:?}");

        let unsupported = dir.path().join("config.ini");
        std::fs::write(&unsupported, "name = \"hello\"\n").unwrap();

        let err = Config::<AppConfig>::load(&unsupported)
            .err()
            .expect("unsupported extension");
        assert!(
            matches!(err, ConfigError::UnsupportedExtension(_)),
            "{err:?}"
        );
    }

    #[test]
    fn load_with_requires_at_least_one_source() {
        let (_dir, path) = config_file(&toml("hello", 8080));

        let err = Config::<AppConfig>::load_with(ConfigOptions::default())
            .err()
            .expect("no source");
        assert!(matches!(err, ConfigError::NoSource), "{err:?}");

        // A path alone configures no source.
        let err = Config::<AppConfig>::load_with(ConfigOptions::new(&path))
            .err()
            .expect("no source");
        assert!(matches!(err, ConfigError::NoSource), "{err:?}");

        // A file source without a path is a different mistake.
        let err = Config::<AppConfig>::load_with(ConfigOptions::default().file())
            .err()
            .expect("no path");
        assert!(matches!(err, ConfigError::MissingPath), "{err:?}");
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Layered {
        from_defaults: String,
        from_file: String,
        from_env: String,
        from_merge: String,
    }

    /// A hand-written provider, as an application would write one for its own
    /// legacy flat variables.
    struct MergeLayer;

    impl Provider for MergeLayer {
        fn metadata(&self) -> Metadata {
            Metadata::named("test merge layer")
        }

        fn data(&self) -> Result<ProfileMap<Profile, Dict>, figment::Error> {
            let mut values = Dict::new();
            values.insert(
                "from_merge".to_owned(),
                figment::value::Value::from("merge"),
            );

            let mut profiles = ProfileMap::new();
            profiles.insert(Profile::Default, values);
            Ok(profiles)
        }
    }

    #[test]
    fn sources_are_merged_in_order_with_later_ones_winning() {
        const PREFIX: &str = "VIVARIUM_CONFIG_TEST_LAYERS";

        let (_dir, path) =
            config_file("from_file = \"file\"\nfrom_env = \"file\"\nfrom_merge = \"file\"\n");
        // SAFETY: this test only touches variables under its own unique prefix.
        unsafe {
            std::env::set_var(format!("{PREFIX}__FROM_ENV"), "env");
            std::env::set_var(format!("{PREFIX}__FROM_MERGE"), "env");
        }

        let config = Config::<Layered>::load_with(
            ConfigOptions::new(&path)
                .defaults(&Layered {
                    from_defaults: "defaults".to_owned(),
                    from_file: "defaults".to_owned(),
                    from_env: "defaults".to_owned(),
                    from_merge: "defaults".to_owned(),
                })
                .file()
                .env_prefixed(PREFIX)
                .separator("__")
                .merge(MergeLayer),
        )
        .expect("layered load");

        let value = config.get();
        assert_eq!(value.from_defaults, "defaults");
        assert_eq!(value.from_file, "file");
        assert_eq!(value.from_env, "env");
        assert_eq!(value.from_merge, "merge");

        // SAFETY: as above.
        unsafe {
            std::env::remove_var(format!("{PREFIX}__FROM_ENV"));
            std::env::remove_var(format!("{PREFIX}__FROM_MERGE"));
        }
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Nested {
        auth: Auth,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Auth {
        jwt: Jwt,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Jwt {
        secret: String,
    }

    #[test]
    fn env_prefixed_nests_variables_on_the_separator() {
        const PREFIX: &str = "VIVARIUM_CONFIG_TEST_NESTED";
        // SAFETY: this test only touches variables under its own unique prefix.
        unsafe { std::env::set_var(format!("{PREFIX}__AUTH__JWT__SECRET"), "s3cret") };

        let config = Config::<Nested>::load_with(
            ConfigOptions::default()
                .env_prefixed(PREFIX)
                .separator("__"),
        )
        .expect("environment-only load");

        assert_eq!(config.get().auth.jwt.secret, "s3cret");

        // SAFETY: as above.
        unsafe { std::env::remove_var(format!("{PREFIX}__AUTH__JWT__SECRET")) };
    }

    #[test]
    fn reload_calls_handlers_once_in_order_with_new_values() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config: Config<AppConfig> = Config::load(&path).unwrap();

        let calls: Arc<Mutex<Vec<(usize, String)>>> = Arc::new(Mutex::new(Vec::new()));

        let first = Arc::clone(&calls);
        config.register(move |c: &AppConfig| {
            first.lock().push((0, c.name.clone()));
        });

        let second = Arc::clone(&calls);
        config.register(move |c: &AppConfig| {
            second.lock().push((1, c.name.clone()));
        });

        std::fs::write(&path, toml("new", 2)).unwrap();
        let new_value = config.reload().unwrap();
        assert_eq!(new_value.name, "new");

        let recorded = calls.lock();
        assert_eq!(
            *recorded,
            vec![(0, "new".to_owned()), (1, "new".to_owned())]
        );
    }

    #[test]
    fn failed_reload_reports_through_on_error_and_keeps_the_old_value() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config: Config<AppConfig> = Config::load(&path).unwrap();

        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&errors);
        config.on_error(move |err| recorded.lock().push(err.to_string()));

        let updates = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&updates);
        config.register(move |_: &AppConfig| {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        std::fs::write(&path, "not valid toml [[[").unwrap();
        let err = config.reload().expect_err("reload fails");
        assert!(matches!(err, ConfigError::Figment(_)), "{err:?}");

        assert_eq!(config.get().name, "old");
        assert_eq!(updates.load(Ordering::SeqCst), 0);
        assert_eq!(errors.lock().len(), 1);
    }

    #[test]
    fn identical_reload_is_skipped_unless_always_notify() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config: Config<AppConfig> = Config::load(&path).unwrap();

        let updates = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&updates);
        config.register(move |_: &AppConfig| {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        // Same values, different file text: no handler runs.
        std::fs::write(&path, format!("# rewritten\n{}", toml("old", 1))).unwrap();
        config.reload().unwrap();
        assert_eq!(updates.load(Ordering::SeqCst), 0);

        // ... unless the comparison is turned off.
        config.always_notify();
        config.reload().unwrap();
        assert_eq!(updates.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unregister_detaches_update_and_error_handlers() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config: Config<AppConfig> = Config::load(&path).unwrap();

        let calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&calls);
        let first_id = config.register(move |_: &AppConfig| first.lock().push("first"));
        let second = Arc::clone(&calls);
        config.register(move |_: &AppConfig| second.lock().push("second"));

        let errors = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&errors);
        let error_id = config.on_error(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        // A failing reload reaches the error handler and no update handler.
        std::fs::write(&path, "broken [[[").unwrap();
        config.reload().unwrap_err();
        assert_eq!(errors.load(Ordering::SeqCst), 1);
        assert!(calls.lock().is_empty());

        // A successful reload reaches both update handlers.
        std::fs::write(&path, toml("new", 2)).unwrap();
        config.reload().unwrap();
        assert_eq!(*calls.lock(), vec!["first", "second"]);

        assert!(config.unregister(first_id));
        assert!(!config.unregister(first_id));
        assert!(config.unregister(error_id));
        assert!(!config.unregister(error_id));

        // Only the second handler and no error handler remain.
        std::fs::write(&path, toml("newer", 3)).unwrap();
        config.reload().unwrap();
        assert_eq!(*calls.lock(), vec!["first", "second", "second"]);

        std::fs::write(&path, "still broken [[[").unwrap();
        config.reload().unwrap_err();
        assert_eq!(errors.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watch_does_not_reload_before_a_change() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());

        let updates = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&updates);
        config.register(move |_: &AppConfig| {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        let _watcher = Arc::clone(&config).watch().unwrap();
        std::thread::sleep(Duration::from_millis(250));

        assert_eq!(updates.load(Ordering::SeqCst), 0);
        assert_eq!(config.get().name, "old");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watch_skips_identical_rewrites() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());

        let (updates, observed) = mpsc::channel();
        config.register(move |c: &AppConfig| {
            let _ = updates.send(c.name.clone());
        });

        let _watcher = Arc::clone(&config).watch().unwrap();

        // Same values, different file text.
        std::fs::write(&path, format!("# rewritten\n{}", toml("old", 1))).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_millis(500)),
            Err(RecvTimeoutError::Timeout)
        );

        // The watcher was alive all along; a real change still gets through.
        std::fs::write(&path, toml("new", 2)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(10)).unwrap(),
            "new"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stop_ends_the_watcher_thread() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());

        let (updates, observed) = mpsc::channel();
        config.register(move |c: &AppConfig| {
            let _ = updates.send(c.name.clone());
        });

        let watcher = Arc::clone(&config).watch().unwrap();

        std::fs::write(&path, toml("new", 2)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(10)).unwrap(),
            "new"
        );

        watcher.stop().unwrap();

        // The thread is gone, so further writes cannot reload anything.
        std::fs::write(&path, toml("later", 3)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_millis(300)),
            Err(RecvTimeoutError::Timeout)
        );
        assert_eq!(config.get().name, "new");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_watcher_ends_the_watcher_thread() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());

        let (updates, observed) = mpsc::channel();
        config.register(move |c: &AppConfig| {
            let _ = updates.send(c.name.clone());
        });

        let watcher = Arc::clone(&config).watch().unwrap();

        std::fs::write(&path, toml("new", 2)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(10)).unwrap(),
            "new"
        );

        drop(watcher);

        std::fs::write(&path, toml("later", 3)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_millis(300)),
            Err(RecvTimeoutError::Timeout)
        );
        assert_eq!(config.get().name, "new");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn panic_in_a_handler_is_reported_and_watching_continues() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());

        config.register(|_: &AppConfig| panic!("handler exploded"));

        let (updates, observed) = mpsc::channel();
        config.register(move |c: &AppConfig| {
            let _ = updates.send(c.name.clone());
        });

        let (errors, reported) = mpsc::channel();
        config.on_error(move |err| {
            let _ = errors.send(err.to_string());
        });

        let _watcher = Arc::clone(&config).watch().unwrap();

        std::fs::write(&path, toml("second", 2)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(10)).unwrap(),
            "second"
        );

        let panic = reported.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(panic.contains("handler exploded"), "{panic}");

        // The watcher survived both the panic and the failed handler.
        std::fs::write(&path, toml("third", 3)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(10)).unwrap(),
            "third"
        );
    }

    /// A user provider that panics on every `data()` call until the test heals
    /// it. Call counting would be platform-dependent: one write can deliver more
    /// than one watcher event, so "panic on the second call" lets a later event
    /// reload successfully while the test still expects failures.
    struct PanickingProvider {
        healthy: Arc<AtomicBool>,
    }

    impl PanickingProvider {
        fn new() -> (Self, Arc<AtomicBool>) {
            let healthy = Arc::new(AtomicBool::new(true));
            (
                Self {
                    healthy: Arc::clone(&healthy),
                },
                healthy,
            )
        }
    }

    impl Provider for PanickingProvider {
        fn metadata(&self) -> Metadata {
            Metadata::named("test panicking provider")
        }

        fn data(&self) -> Result<ProfileMap<Profile, Dict>, figment::Error> {
            assert!(self.healthy.load(Ordering::SeqCst), "provider exploded");

            let mut values = Dict::new();
            values.insert(
                "provider_marker".to_owned(),
                figment::value::Value::from("ok"),
            );

            let mut profiles = ProfileMap::new();
            profiles.insert(Profile::Default, values);
            Ok(profiles)
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn panic_in_a_provider_is_reported_and_watching_continues() {
        let (_dir, path) = config_file(&toml("old", 1));
        let (provider, healthy) = PanickingProvider::new();
        let options = ConfigOptions::new(&path).file().merge(provider);
        let config = Arc::new(Config::<AppConfig>::load_with(options).unwrap());

        let (updates, observed) = mpsc::channel();
        config.register(move |c: &AppConfig| {
            let _ = updates.send(c.name.clone());
        });

        let (errors, reported) = mpsc::channel();
        config.on_error(move |err| {
            let is_handler_panic = matches!(err, ConfigError::HandlerPanic(_));
            let _ = errors.send((is_handler_panic, err.to_string()));
        });

        let _watcher = Arc::clone(&config).watch().unwrap();

        // Every reload panics from here on: a single write can deliver more than
        // one file-system event, and each of them has to be caught.
        healthy.store(false, Ordering::SeqCst);
        std::fs::write(&path, toml("second", 2)).unwrap();
        let (is_handler_panic, panic) = reported.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(panic.contains("provider exploded"), "{panic}");
        assert!(is_handler_panic, "{panic}");

        // Drive a second reload through the same path ...
        std::fs::write(&path, toml("second", 2)).unwrap();

        // ... and no update handler may run while the provider keeps panicking,
        // nor may the failed reloads replace the value.
        assert_eq!(
            observed.recv_timeout(Duration::from_millis(300)),
            Err(RecvTimeoutError::Timeout)
        );
        assert_eq!(config.get().name, "old");

        // A healed provider proves the watcher thread survived and still watches.
        // An event queued by the writes above may land first, so drain until the
        // healed value arrives.
        healthy.store(true, Ordering::SeqCst);
        std::fs::write(&path, toml("third", 3)).unwrap();
        let mut saw_third = false;
        for _ in 0..20 {
            match observed.recv_timeout(Duration::from_millis(500)) {
                Ok(name) if name == "third" => {
                    saw_third = true;
                    break;
                }
                Ok(_) | Err(_) => {}
            }
        }
        assert!(
            saw_third,
            "the healed reload never reached the update handler"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_reload_is_reported_and_watching_continues() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());

        let (updates, observed) = mpsc::channel();
        config.register(move |c: &AppConfig| {
            let _ = updates.send(c.name.clone());
        });

        let (errors, reported) = mpsc::channel();
        config.on_error(move |err| {
            let _ = errors.send(err.to_string());
        });

        let _watcher = Arc::clone(&config).watch().unwrap();

        std::fs::write(&path, "not valid toml [[[").unwrap();
        let error = reported.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(error.contains("failed to parse"), "{error}");
        assert_eq!(config.get().name, "old");

        std::fs::write(&path, toml("recovered", 2)).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(10)).unwrap(),
            "recovered"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn poll_interval_opts_into_the_polling_watcher() {
        let (_dir, path) = config_file(&toml("old", 1));
        let config = Arc::new(Config::<AppConfig>::load(&path).unwrap());

        let (updates, observed) = mpsc::channel();
        config.register(move |c: &AppConfig| {
            let _ = updates.send(c.name.clone());
        });

        config.poll_interval(Duration::from_millis(50));
        let _watcher = Arc::clone(&config).watch().unwrap();

        // Polling compares modification times, which are coarse; make sure the
        // rewrite lands in a later time bucket than the original file.
        std::thread::sleep(Duration::from_millis(1200));
        std::fs::write(&path, toml("polled", 2)).unwrap();

        assert_eq!(
            observed.recv_timeout(Duration::from_secs(10)).unwrap(),
            "polled"
        );
    }
}
