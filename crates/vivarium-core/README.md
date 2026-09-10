# vivarium-core

Core value types of the [vivarium](https://github.com/ManhattanCafeee/Vivarium)
family: pagination, ordering, result pages, bindable values, and the `Entity`
contract — the shared vocabulary between `vivarium-db` and `vivarium-web`.

> The code snippets in this file are illustrative and are not compiled by CI;
> the compiled examples live in the root README.

## What's inside

- [`Pagination`] — 1-based page requests; construction normalizes out-of-range
  values (`page <= 1_000_000`, `per_page <= 100`, zero becomes 20)
- [`Order`] / [`Column`] / [`Sorter`] — a compile-time sorting whitelist:
  column names can only come from your `Column` enum, never from a string
- [`Page<T>`] — paged results (`items`, `total`, `page`, `per_page`)
- [`Entity`] — the table-mapping contract behind the generic CRUD of
  `vivarium-db` (usually derived: `#[derive(vivarium_rs::Entity)]`), with a
  typed [`PrimaryKey`] (`i64`, `u64`, `i32`, `u32`, `String`)
- [`Value`] — driver-agnostic bindable column values used by the CRUD layer,
  including typed `NULL`s ([`NullType`]) and optional `chrono`/`uuid` variants

## Features

| Feature | Adds |
|---|---|
| `chrono` | `Value::DateTime` / `Value::NaiveDate` |
| `uuid` | `Value::Uuid` |
| `utoipa` | `ToSchema` for `Page`/`Pagination` and `IntoParams` for `Pagination` |

## Quick start

```rust
use vivarium_core::Pagination;

let p = Pagination::new(0, 0); // normalized: page 1, per_page 20
assert_eq!((p.page, p.per_page), (1, 20));
assert_eq!(p.limit_offset(), (20, 0));
```

```rust
use vivarium_core::{Column, Order, Sorter};

#[derive(Clone, Copy)]
enum UserCol { Name }

impl Column for UserCol {
    fn name(&self) -> &'static str {
        match self { UserCol::Name => "name" }
    }
}

let sorter = Sorter::new(UserCol::Name, Order::Asc);
```

MSRV: Rust 1.94.

[`Pagination`]: https://docs.rs/vivarium-core/latest/vivarium_core/struct.Pagination.html
[`Order`]: https://docs.rs/vivarium-core/latest/vivarium_core/enum.Order.html
[`Column`]: https://docs.rs/vivarium-core/latest/vivarium_core/trait.Column.html
[`Page<T>`]: https://docs.rs/vivarium-core/latest/vivarium_core/struct.Page.html
[`PrimaryKey`]: https://docs.rs/vivarium-core/latest/vivarium_core/trait.PrimaryKey.html
[`Sorter`]: https://docs.rs/vivarium-core/latest/vivarium_core/struct.Sorter.html
[`Entity`]: https://docs.rs/vivarium-core/latest/vivarium_core/trait.Entity.html
[`Value`]: https://docs.rs/vivarium-core/latest/vivarium_core/enum.Value.html
[`NullType`]: https://docs.rs/vivarium-core/latest/vivarium_core/enum.NullType.html
