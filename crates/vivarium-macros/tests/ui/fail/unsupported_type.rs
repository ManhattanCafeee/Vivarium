//! Fail case: unsupported field type without #[entity(json)].
use vivarium_macros::Entity;

struct Custom;

#[derive(Entity)]
struct User {
    id: i64,
    custom: Custom,
}

fn main() {}
