//! Fail case: id field must be i64.
use vivarium_macros::Entity;

#[derive(Entity)]
struct User {
    id: i32,
    name: String,
}

fn main() {}
