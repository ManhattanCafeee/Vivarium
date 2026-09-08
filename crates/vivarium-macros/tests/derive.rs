//! Behavior tests for the `#[derive(Entity)]` expansion.
use serde_json::Value;
use vivarium_core::Entity;

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
    let pairs = account.columns_and_values();
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
    assert_eq!(user.columns_and_values().len(), 1);
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
    let pairs = customer.columns_and_values();
    assert_eq!(pairs[0].0, "display_name");
}

// Option None → Value::Null
#[test]
fn option_none_maps_to_null() {
    let account = Account {
        account_id: 7,
        name: "n".into(),
        age: None,
        metadata: Value::Null,
        internal: "i".into(),
        active: true,
    };
    let pairs = account.columns_and_values();
    let age = pairs.iter().find(|(col, _)| *col == "age").map(|(_, v)| v);
    assert_eq!(age, Some(&vivarium_core::Value::Null));
}
