//! Fail case: no id field.
use vivarium_macros::Entity;

#[derive(Entity)]
struct User {
    name: String,
}

fn main() {}
