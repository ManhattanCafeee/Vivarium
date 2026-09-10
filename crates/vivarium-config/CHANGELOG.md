# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0 — unreleased

### Breaking

- `Config<T>` now also requires `T: Serialize` (reloads compare serialized
  values to skip no-op reloads; disable with `.always_notify()`).
- `Config::watch` returns a `ConfigWatcher` instead of `()`.
- Handler registration returns a `HandlerId`, and `ConfigError` gained
  variants (`NoSource`, `MissingPath`, `HandlerPanic`,
  `WatcherThreadPanicked`, …).

### Added

- `ConfigOptions` + `Config::load_with`: an ordered source stack
  (defaults → file → prefixed environment → user `figment::Provider`s), with
  `ConfigError::NoSource` when nothing is configured.
- `Config::register`, `Config::unregister`, `Config::on_error`, and
  `HandlerId`.
- `ConfigWatcher::stop` plus `Drop` (the watcher thread is signalled and
  joined, never leaked).
- `Config::poll_interval` to opt into `PollWatcher`; the default is now the
  native `notify::RecommendedWatcher`.

### Changed

- Reload failures and watcher errors are reported through `on_error` and
  `tracing::warn!` instead of being written to stderr.
- A panicking handler is caught (`catch_unwind`), reported through
  `on_error`, and does not stop watching.
- Value de-duplication: a reload whose serialized value is unchanged calls no
  handlers.
- `Config::load` keeps its previous signature and semantics.

### Documentation

- README: source precedence, hot-reload behaviour, limitations, and the
  mapping from a flat `HOSHIYOMI__*` configuration.
