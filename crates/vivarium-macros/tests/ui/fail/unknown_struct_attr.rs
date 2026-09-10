//! Fail case: an unknown struct-level attribute is rejected instead of ignored.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(tabl = "users")]
struct User {
    id: i64,
    name: String,
}

fn main() {}
