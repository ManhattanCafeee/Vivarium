//! Fail case: the id field must be a primary-key type.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    id: f64,
    name: String,
}

fn main() {}
