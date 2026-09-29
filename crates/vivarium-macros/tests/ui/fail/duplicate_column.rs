//! Fail case: a non-id field renamed onto the id column is rejected instead of
//! producing `INSERT INTO t ("id", "id") …`.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    id: i64,
    #[entity(rename = "id")]
    name: String,
}

fn main() {}
