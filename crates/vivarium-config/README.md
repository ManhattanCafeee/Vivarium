# vivarium-config

Typed, hot-reloadable configuration for the
[vivarium](https://github.com/ManhattanCafeee/Vivarium) family. Built on
[figment](https://github.com/SergioBenitez/Figment) (toml/yaml/json),
[notify](https://github.com/notify-rs/notify) (file watching), and
[arc-swap](https://github.com/vorner/arc-swap) (lock-free reads).

## Quick start

```rust,ignore
use std::sync::Arc;
use serde::Deserialize;
use vivarium_config::Config;

#[derive(Deserialize)]
struct AppConfig {
    port: u16,
}

// config.toml:  port = 8080
let config = Arc::new(Config::load("config.toml")?);
config.register(|cfg: &AppConfig| println!("port is now {}", cfg.port));
config.clone().watch()?; // watches the file; re-loads and fires handlers on change

let cfg = config.get(); // lock-free Arc read
```

## What's inside

- `Config::load` — parses by file extension (`.toml` / `.yml` / `.yaml` /
  `.json`)
- `Config::get` — lock-free snapshot (`Arc<T>`)
- `Config::register` — update handlers, called in registration order on
  every successful reload
- `Config::reload` — manual reload; on failure the previous config stays
  active and no handlers run
- `Config::watch` — parent-directory file watcher with debounce (atomic-save
  renames are caught)

MSRV: Rust 1.85.
