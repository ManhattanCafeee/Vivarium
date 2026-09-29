//! Fail case: a blank `table` literal is rejected instead of reaching SQL.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core", table = "")]
struct User {
    id: i64,
}

fn main() {}
