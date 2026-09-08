//! Pass case: custom table, marked id field, rename, json, skip.
use serde_json::Value;
use vivarium_core::Entity;

#[derive(vivarium_macros::Entity)]
#[entity(table = "accounts")]
struct Account {
    #[entity(id)]
    account_id: i64,
    name: String,
    age: Option<i32>,
    #[entity(rename = "meta")]
    metadata: Value,
    #[entity(skip)]
    internal: String,
}

fn main() {
    let account = Account {
        account_id: 7,
        name: "n".into(),
        age: Some(3),
        metadata: Value::Null,
        internal: "i".into(),
    };
    assert_eq!(Account::TABLE, "accounts");
    assert_eq!(Account::ID_COLUMN, "account_id");
    assert_eq!(account.id(), 7);
    let names: Vec<&str> = account.columns_and_values().iter().map(|(c, _)| *c).collect();
    assert_eq!(names, vec!["name", "age", "meta"]);
}
