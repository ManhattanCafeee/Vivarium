# vivarium-core

Core value types of the [vivarium](https://github.com/ManhattanCafeee/Vivarium)
family: pagination, ordering, and result pages — the dependency-free shared
vocabulary between `vivarium-db` and `vivarium-web`.

## What's inside

- [`Pagination`] — 1-based page requests; construction normalizes out-of-range
  values (`page <= 1_000_000`, `size <= 100`, zero size becomes 20)
- [`Order`] / [`Column`] / [`Sorter`] — a compile-time sorting whitelist:
  column names can only come from your `Column` enum, never from a string
- [`Page<T>`] — paged results with total count and page count
- [`Entity`] — the table-mapping contract behind the generic CRUD of
  `vivarium-db` (usually derived: `#[derive(vivarium_rs::Entity)]`)
- [`Value`] — driver-agnostic bindable column values used by the CRUD layer

## Quick start

```rust
use vivarium_core::Pagination;

let p = Pagination::new(0, 0); // normalized: page 1, size 20
assert_eq!((p.page, p.size), (1, 20));
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

This crate has no dependencies. MSRV: Rust 1.85.

[`Pagination`]: https://docs.rs/vivarium-core/latest/vivarium_core/struct.Pagination.html
[`Order`]: https://docs.rs/vivarium-core/latest/vivarium_core/enum.Order.html
[`Column`]: https://docs.rs/vivarium-core/latest/vivarium_core/trait.Column.html
[`Sorter`]: https://docs.rs/vivarium-core/latest/vivarium_core/struct.Sorter.html
[`Page<T>`]: https://docs.rs/vivarium-core/latest/vivarium_core/struct.Page.html
[`Entity`]: https://docs.rs/vivarium-core/latest/vivarium_core/trait.Entity.html
[`Value`]: https://docs.rs/vivarium-core/latest/vivarium_core/enum.Value.html
