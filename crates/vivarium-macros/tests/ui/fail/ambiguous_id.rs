//! Fail case: ambiguous id — a marked field plus a field named `id`.
use vivarium_macros::Entity;

#[derive(Entity)]
struct User {
    #[entity(id)]
    uuid: i64,
    id: i64,
}

fn main() {}
