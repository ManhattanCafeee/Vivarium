//! Fail case: tuple structs are not supported.
use vivarium_macros::Entity;

#[derive(Entity)]
struct Pair(i64, String);

fn main() {}
