# vivarium-macros

Procedural macros of the [vivarium](https://github.com/ManhattanCafeee/Vivarium)
family. Provides `#[derive(Entity)]`, which derives `vivarium_core::Entity` for
single-primary-key structs.

The derive is re-exported by both `vivarium-rs` and `vivarium-db`, so you
normally do not depend on this crate directly.

> The code snippets in this file are illustrative and are not compiled by CI;
> the compiled examples live in the root README.

## Attributes

- `#[entity(table = "name")]` (struct) — table name; defaults to the
  snake_case form of the struct name
- `#[entity(crate = "path")]` (struct) — crate path the generated code
  anchors on (the `Entity` trait and `Value` type). Defaults to
  `::vivarium_rs`; set `"vivarium_db"` when using the derive through
  `vivarium-db` without the facade
- `#[entity(id)]` (field) — primary-key field; defaults to a field named
  `id`. The type must implement `PrimaryKey`: `i64`, `u64`, `i32`, `u32`, or
  `String`
- `#[entity(rename = "col")]` (field) — column name; defaults to the field
  name
- `#[entity(json)]` (field) — serialize the field via `serde_json` into a
  JSON column. Requires `T: Serialize`; a serialization failure is reported as
  `EncodeError` when the row is written, never as a panic
- `#[entity(skip)]` (field) — exclude the field from
  `columns_and_values` (it still decodes in `FromRow`)

Unknown attribute keys are compile errors rather than being ignored, and an
unsupported field type is rejected with the list of supported types.

## Example

```rust
use serde_json::Value;
use vivarium_core::{Entity, NullType};

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

let columns = user.columns_and_values().expect("encodes");
assert_eq!(columns.len(), 3);
// `None` binds as a typed NULL so drivers that check parameter types accept it.
assert_eq!(columns[1].1, vivarium_core::Value::TypedNull(NullType::I64));
```

Supported field types: `i8`–`i64`, `u8`–`u64`, `usize`, `isize`, `f32`,
`f64`, `bool`, `String`, `Vec<u8>`, `serde_json::Value`,
`chrono::DateTime<Utc>`, `chrono::NaiveDate`, `uuid::Uuid`, `Option` of those,
or anything marked `#[entity(json)]`. `chrono` and `uuid` fields additionally
require the matching `vivarium-core` feature (`chrono` / `uuid`).

MSRV: Rust 1.94.
