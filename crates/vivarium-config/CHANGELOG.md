# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.4.0 — 2026-09-29

### Fixed

- Blank values no longer silently disable a source or spin the watcher:
  `ConfigOptions::new("")`, a blank `env_prefixed` prefix, a blank `separator`
  and a zero `Config::poll_interval` are reported when the configuration is
  loaded (or when `watch` starts) as `ConfigError::EmptyPath`,
  `ConfigError::EmptyEnvPrefix`, `ConfigError::EmptyEnvSeparator` and
  `ConfigError::ZeroPollInterval` — four new variants. Previously an empty
  prefix merged the whole environment into the configuration, an empty
  separator made figment drop every prefixed variable, an empty path resolved
  to the working directory (surfacing as a misleading
  `UnsupportedExtension("<none>")`, or a watcher that never matched), and a
  zero interval busy-spun the polling watcher.

## 0.3.4 — 2026-09-14

### Changed

- Version bump only, released in lockstep with the 0.3.4 workspace; no code
  changes.

## 0.3.2 — 2026-09-11

### Changed

- Version bump only, released in lockstep with the 0.3.2 workspace; no code
  changes.

## 0.3.1 — 2026-09-10

### Fixed

- A panicking user provider no longer kills the watcher thread: the reload runs
  under `catch_unwind`, the panic is reported through
  `ConfigError::HandlerPanic` (whose documentation and message now cover
  providers as well as handlers), the previous value stays in place, and
  watching continues. Previously the thread died silently — later file changes
  triggered nothing and `on_error` never fired.

## 0.3.0 — 2026-09-10

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
