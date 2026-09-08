//! Pass case: defaults — snake_case table name, field named `id`.
use vivarium_core::Entity;

#[derive(vivarium_macros::Entity)]
#[entity(crate = "vivarium_core")]
struct UserProfile {
    id: i64,
    name: String,
}

fn main() {
    let profile = UserProfile { id: 1, name: "n".into() };
    assert_eq!(UserProfile::TABLE, "user_profile");
    assert_eq!(UserProfile::ID_COLUMN, "id");
    assert_eq!(profile.id(), 1);
}
