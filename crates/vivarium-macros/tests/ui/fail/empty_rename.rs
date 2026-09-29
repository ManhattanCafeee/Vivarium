//! Fail case: a blank `rename` literal is rejected instead of naming a
//! column `""`.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    id: i64,
    #[entity(rename = "  ")]
    name: String,
}

fn main() {}
