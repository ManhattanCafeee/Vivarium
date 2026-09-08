//! Fail case: generic structs are not supported.
use vivarium_macros::Entity;

#[derive(Entity)]
struct Boxed<T> {
    id: i64,
    value: T,
}

fn main() {}
