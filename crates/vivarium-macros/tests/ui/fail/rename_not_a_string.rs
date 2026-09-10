//! Fail case: a `rename` value that is not a string literal is rejected.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    id: i64,
    #[entity(rename = 12)]
    name: String,
}

fn main() {}
