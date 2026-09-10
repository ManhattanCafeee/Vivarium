//! Fail case: `rename` without a value is rejected instead of ignored.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    id: i64,
    #[entity(rename)]
    name: String,
}

fn main() {}
