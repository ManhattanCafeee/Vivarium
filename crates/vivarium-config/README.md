# vivarium-config

Typed, hot-reloadable configuration for the
[vivarium](https://github.com/ManhattanCafeee/Vivarium) family. Built on
[figment](https://github.com/SergioBenitez/Figment) (layered sources),
[notify](https://github.com/notify-rs/notify) (file watching), and
[arc-swap](https://github.com/vorner/arc-swap) (lock-free reads).

> The code snippets in this file are illustrative and are not compiled by CI;
> the compiled examples live in the root README.

## Quick start

```rust,ignore
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use vivarium_config::{Config, ConfigOptions};

#[derive(Debug, Deserialize, Serialize)]
struct AppConfig {
    port: u16,
}

let config = Arc::new(Config::load_with(
    ConfigOptions::new("config.toml")
        .defaults(&AppConfig { port: 8080 })   // 1. code defaults
        .file()                                // 2. config.toml (optional file)
        .env_prefixed("APP")                   // 3. APP__* environment variables
        .separator("__"),
)?);

config.register(|cfg: &AppConfig| println!("port is now {}", cfg.port));
config.on_error(|err| eprintln!("config error: {err}"));

let watcher = Arc::clone(&config).watch()?; // hot reload
let cfg = config.get();                     // lock-free Arc<T> snapshot
drop(watcher);                              // stops and joins the watcher
```

## Sources and precedence

`ConfigOptions` is a builder over an ordered stack. Each source overrides the
values of the ones before it, and the stack is re-evaluated on every reload:

| Order | Builder call | Notes |
|---|---|---|
| 1 | `ConfigOptions::defaults(&value)` | serialized once with `serde`; needs no `Clone` |
| 2 | `ConfigOptions::file()` / `ConfigOptions::file_required()` | format picked by extension (`.toml` / `.yml` / `.yaml` / `.json`); `file()` tolerates a missing file, `file_required()` reports `ConfigError::Io` |
| 3 | `ConfigOptions::env_prefixed("APP").separator("__")` | `APP__AUTH__JWT__SECRET` becomes `auth.jwt.secret`; the prefix and the separator are stripped |
| 4 | `ConfigOptions::merge(provider)` | any `figment::Provider`; in call order, so the last call wins |

At least one source is required, otherwise `Config::load_with` returns
`ConfigError::NoSource`. `Config::load(path)` is `Config::load_with` with a
single required file source and keeps its original semantics.

```rust,ignore
/// Application-side provider: maps legacy flat variables onto the new shape.
struct LegacyFlatEnv;

impl figment::Provider for LegacyFlatEnv {
    fn metadata(&self) -> figment::Metadata {
        figment::Metadata::named("legacy flat environment")
    }

    fn data(
        &self,
    ) -> Result<figment::value::Map<figment::Profile, figment::value::Dict>, figment::Error> {
        // figment only expands dotted keys in providers that call
        // `figment::util::nest` (its `Env` provider does); a `Dict` returned
        // from a custom provider is merged verbatim, so nested values must be
        // built explicitly or the field would never be populated.
        let mut values = figment::value::Dict::new();
        if let Ok(port) = std::env::var("HOSHIYOMI_PORT") {
            values.insert("port".to_owned(), port.into());
        }
        if let Ok(secret) = std::env::var("HOSHIYOMI_JWT_SECRET") {
            let mut jwt = figment::value::Dict::new();
            jwt.insert("secret".to_owned(), secret.into());
            let mut auth = figment::value::Dict::new();
            auth.insert("jwt".to_owned(), jwt.into());
            values.insert("auth".to_owned(), auth.into());
        }

        let mut profiles = figment::value::Map::new();
        profiles.insert(figment::Profile::Default, values);
        Ok(profiles)
    }
}
```

## API

- `Config::load` / `Config::load_with` — build the initial value from a source
  stack.
- `Config::get` — lock-free `Arc<T>` snapshot, valid even after a reload.
- `Config::reload` — re-evaluate the stack; the previous value is kept if any
  source fails.
- `Config::watch` — watch the config file and reload on change; returns a
  `ConfigWatcher` that stops (and joins) the watcher thread on drop.
- `Config::register` / `Config::unregister` — update handlers and their
  `HandlerId`s, called in registration order on notifying reloads.
- `Config::on_error` — error handlers; reload failures, watcher errors, and
  handler panics are reported here and logged with `tracing::warn!`.
- `Config::always_notify` — notify handlers even for an unchanged value.
- `Config::poll_interval` — use a polling watcher instead of the native one.

## Hot reload

`watch()` observes the config file's parent directory non-recursively (so
atomic-save renames are caught), ignores events for other files, and debounces
bursts of events before a single reload. Reload failures are reported through
`on_error` and watching continues. A reload whose serialized value equals the
current one calls no handlers unless `always_notify()` was set. Handlers run on
the watcher thread, wrapped in `catch_unwind`: a panicking handler is reported
through `on_error` and does not stop watching.

## Limitations

- **Synchronous and thread-based.** There is no async/tokio integration;
  `watch()` starts one OS thread per call, and handlers run on the thread that
  performed the reload.
- **No initial reload.** `watch()` does not reload on start: the value read
  right after it is the one produced by `load`/`load_with`. The first reload
  happens on the first observed file change.
- **One file, non-recursively watched.** Only the configured file is watched
  (through its parent directory). Includes, directory trees, and files
  referenced from the config are not followed.
- **Handlers are sequential.** A slow handler delays reloads; `stop()` and
  `Drop` wait for the handler in flight to finish.
- **De-duplication is a value comparison.** It needs `T: Serialize`, compares
  the serialized form of the whole value (so a change in a field always
  notifies, and an unorderable value such as a `NaN` always notifies), and is
  disabled by `always_notify()`.
- **All-or-nothing reloads.** A reload either succeeds completely or leaves the
  previous value in place; there is no per-field update and no rollback of
  handlers that already ran.
- **No filesystem layout decisions.** The library loads exactly the path it is
  given; local-mode layouts, search paths, and directory conventions stay in
  the application.
- **Polling is coarse.** With `poll_interval()`, changes that stay inside one
  modification-time bucket can be missed; native events are the default.
- **No schema validation beyond extraction.** Validation is whatever `serde`
  accepts while deserializing `T`.

## Migrating from a flat `HOSHIYOMI__*` config

| hoshiyomi today | vivarium-config 0.3 |
|---|---|
| `RawAppConfig::default()` | `ConfigOptions::defaults(&RawAppConfig::default())` |
| `config.toml` (optional file) | `ConfigOptions::file()` — a missing file is not an error |
| `HOSHIYOMI__` + `__` variables | `ConfigOptions::env_prefixed("HOSHIYOMI").separator("__")` |
| the 9 legacy flat variables | an application-side `figment::Provider` passed to `.merge(..)` |
| local mode / path resolution (`config/paths.rs`) | stays in the application — the library makes no filesystem layout decisions |
| no hot reload | `Config::watch()` + the returned `ConfigWatcher` |

MSRV: Rust 1.94.
