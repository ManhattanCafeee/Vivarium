# vivarium-macros

Procedural macros of the [vivarium](https://github.com/ManhattanCafeee/Vivarium)
family. Provides `#[derive(Entity)]`, which derives `vivarium_core::Entity`
for single-`i64`-primary-key structs.

The derive is re-exported by both `vivarium-rs` and `vivarium-db`, so you
normally do not depend on this crate directly.

## Attributes

- `#[entity(table = "name")]` (struct) — table name; defaults to the
  snake_case form of the struct name
- `#[entity(crate = "path")]` (struct) — crate path the generated code
  anchors on (the `Entity` trait and `Value` type). Defaults to
  `::vivarium_rs`; set `"vivarium_db"` when using the derive through
  `vivarium-db` without the facade
- `#[entity(id)]` (field) — primary-key field; defaults to a field named
  `id`. Must be `i64`
- `#[entity(rename = "col")]` (field) — column name; defaults to the field
  name
- `#[entity(json)]` (field) — serialize the field via `serde_json` into a
  JSON column
- `#[entity(skip)]` (field) — exclude the field from
  `columns_and_values` (it still decodes in `FromRow`)

## Example

```rust
use serde_json::Value;
use vivarium_core::Entity;

#[derive(vivarium_macros::Entity)]
#[entity(table = "users", crate = "vivarium_core")]
struct User {
    id: i64,
    name: String,
    age: Option<i32>,
    #[entity(rename = "meta")]
    metadata: Value,
}

let user = User { id: 7, name: "n".into(), age: None, metadata: Value::Null };
assert_eq!(User::TABLE, "users");
assert_eq!(user.id(), 7);
assert_eq!(user.columns_and_values().len(), 3);
```

Supported field types: `i8`–`i64`, `u8`–`u64`, `usize`, `isize`, `f32`,
`f64`, `bool`, `String`, `Vec<u8>`, `serde_json::Value`, `Option` of those,
or anything marked `#[entity(json)]`.

MSRV: Rust 1.85.
