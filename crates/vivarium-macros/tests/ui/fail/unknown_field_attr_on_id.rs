//! Fail case: an unknown attribute on the id field is reported as an unknown
//! attribute, not swallowed by the id-candidate search.
use vivarium_macros::Entity;

#[derive(Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    // Marked as the id and deliberately *not* named `id`: without validating
    // every field's attributes up front, the typo would disqualify this
    // candidate silently (`is_ok_and` swallows the parse error) and the derive
    // would report a missing id field instead of the attribute mistake.
    #[entity(id, renam = "user_id")]
    user_id: i64,
    name: String,
}

fn main() {}
