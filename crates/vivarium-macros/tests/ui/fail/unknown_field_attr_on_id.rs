//! Fail case: an unknown attribute on the id field is reported as an unknown
//! attribute, not swallowed by the id-candidate search.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    #[entity(id, renam = "user_id")]
    id: i64,
    name: String,
}

fn main() {}
