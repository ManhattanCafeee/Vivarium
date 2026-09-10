//! Behavior tests for the `#[derive(Entity)]` expansion.
use std::collections::HashMap;

use serde_json::Value;
use vivarium_core::{Entity, NullType, PrimaryKey};

#[derive(vivarium_macros::Entity)]
#[entity(table = "accounts", crate = "vivarium_core")]
struct Account {
    #[entity(id)]
    account_id: i64,
    name: String,
    age: Option<i32>,
    #[entity(rename = "meta")]
    metadata: Value,
    #[entity(skip)]
    internal: String,
    active: bool,
}

#[test]
fn custom_table_and_id_field() {
    assert_eq!(Account::TABLE, "accounts");
    assert_eq!(Account::ID_COLUMN, "account_id");
    let account = Account {
        account_id: 7,
        name: "n".into(),
        age: None,
        metadata: Value::Null,
        internal: "i".into(),
        active: true,
    };
    assert_eq!(account.id(), 7);
}

#[test]
fn columns_and_values_skip_id_and_skipped_fields() {
    let account = Account {
        account_id: 7,
        name: "n".into(),
        age: Some(3),
        metadata: Value::String("m".into()),
        internal: "i".into(),
        active: false,
    };
    assert_eq!(account.internal, "i");
    let pairs = account.columns_and_values().expect("encodes");
    let names: Vec<&str> = pairs.iter().map(|(col, _)| *col).collect();
    // id and #[entity(skip)] excluded; rename applied
    assert_eq!(names, vec!["name", "age", "meta", "active"]);
}

#[derive(vivarium_macros::Entity)]
#[entity(crate = "vivarium_core")]
struct User {
    id: i64,
    name: String,
}

#[test]
fn default_table_is_snake_case_and_default_id_field() {
    assert_eq!(User::TABLE, "user");
    assert_eq!(User::ID_COLUMN, "id");
    let user = User {
        id: 1,
        name: "u".into(),
    };
    assert_eq!(user.id(), 1);
    assert_eq!(user.columns_and_values().expect("encodes").len(), 1);
}

#[derive(vivarium_macros::Entity)]
#[entity(table = "customers", crate = "vivarium_core")]
struct CamelCaseCustomer {
    id: i64,
    display_name: String,
}

#[test]
fn snake_case_fields_keep_names() {
    assert_eq!(CamelCaseCustomer::TABLE, "customers");
    let customer = CamelCaseCustomer {
        id: 1,
        display_name: "d".into(),
    };
    let pairs = customer.columns_and_values().expect("encodes");
    assert_eq!(pairs[0].0, "display_name");
}

#[test]
fn option_none_maps_to_typed_null() {
    let account = Account {
        account_id: 7,
        name: "n".into(),
        age: None,
        metadata: Value::Null,
        internal: "i".into(),
        active: true,
    };
    let pairs = account.columns_and_values().expect("encodes");
    let age = pairs.iter().find(|(col, _)| *col == "age").map(|(_, v)| v);
    assert_eq!(age, Some(&vivarium_core::Value::TypedNull(NullType::I64)));
}

#[derive(vivarium_macros::Entity)]
#[entity(table = "sessions", crate = "vivarium_core")]
struct SessionRow {
    #[entity(id)]
    session_id: u64,
    token: String,
}

#[test]
fn u64_primary_key_is_supported() {
    let row = SessionRow {
        session_id: 42,
        token: "t".into(),
    };
    assert_eq!(row.id(), 42_u64);
    assert_eq!(SessionRow::ID_COLUMN, "session_id");
    assert_eq!(row.columns_and_values().expect("encodes").len(), 1);

    // A key above i64::MAX is rejected instead of wrapping silently.
    let out_of_range = SessionRow {
        session_id: u64::MAX,
        token: "t".into(),
    };
    assert!(out_of_range.id().into_value().is_err());
}

#[derive(vivarium_macros::Entity)]
#[entity(table = "kv", crate = "vivarium_core")]
struct Kv {
    #[entity(id)]
    key: String,
    value: String,
}

#[test]
fn string_primary_key_is_supported() {
    let row = Kv {
        key: "a".into(),
        value: "b".into(),
    };
    assert_eq!(row.id(), "a");
    assert!(!row.id().is_unset());
    assert_eq!(
        row.columns_and_values().expect("encodes")[0].1,
        vivarium_core::Value::Text("b".into())
    );
}

#[derive(vivarium_macros::Entity)]
#[entity(table = "events", crate = "vivarium_core")]
struct Event {
    id: i64,
    #[entity(json)]
    payload: HashMap<(i32, i32), i32>,
}

#[test]
fn json_encode_failure_is_an_error_not_a_panic() {
    let mut payload = HashMap::new();
    payload.insert((1, 2), 3);
    let event = Event { id: 1, payload };
    let error = event
        .columns_and_values()
        .expect_err("tuple keys cannot be represented in JSON");
    assert!(
        error.to_string().contains("failed to serialize"),
        "unexpected error: {error}"
    );
}

#[derive(vivarium_macros::Entity)]
#[entity(crate = "vivarium_core")]
struct MarkedId {
    // The marker on the field that is also named `id` is one candidate, not
    // an ambiguity.
    #[entity(id)]
    id: i64,
    name: String,
}

#[test]
fn id_marker_on_the_id_field_is_accepted() {
    let row = MarkedId {
        id: 4,
        name: "n".into(),
    };
    assert_eq!(row.id(), 4);
    assert_eq!(MarkedId::ID_COLUMN, "id");
    assert_eq!(row.columns_and_values().expect("encodes").len(), 1);
}

#[derive(vivarium_macros::Entity)]
#[entity(crate = "vivarium_core")]
struct Blob {
    id: i64,
    data: Vec<u8>,
    optional: Option<Vec<u8>>,
}

#[test]
fn byte_vectors_map_to_bytes_and_typed_null() {
    let row = Blob {
        id: 1,
        data: vec![7, 8],
        optional: None,
    };
    let columns = row.columns_and_values().expect("encodes");
    assert_eq!(columns[0].1, vivarium_core::Value::Bytes(vec![7, 8]));
    assert_eq!(
        columns[1].1,
        vivarium_core::Value::TypedNull(NullType::Bytes)
    );
}
