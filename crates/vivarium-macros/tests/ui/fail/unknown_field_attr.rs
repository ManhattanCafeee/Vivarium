//! Fail case: unknown field attributes are rejected instead of ignored.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    id: i64,
    #[entity(renmae = "username")]
    name: String,
}

fn main() {}
