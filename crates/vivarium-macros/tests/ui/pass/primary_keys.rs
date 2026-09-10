//! Pass case: non-`i64` primary keys, typed NULLs, and JSON fields.
use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use uuid::Uuid;
use vivarium_core::{Entity, NullType, PrimaryKey, Value as CoreValue};

#[derive(vivarium_macros::Entity)]
#[entity(table = "sessions", crate = "vivarium_core")]
struct SessionRow {
    #[entity(id)]
    session_id: String,
    user_id: u64,
    expires_at: Option<i64>,
    payload: Option<Value>,
    #[entity(json)]
    meta: BTreeMap<String, String>,
}

#[derive(vivarium_macros::Entity)]
#[entity(crate = "vivarium_core")]
struct Ticket {
    id: u32,
    #[entity(rename = "kind")]
    kind: String,
    #[entity(skip)]
    cached: bool,
}

#[derive(vivarium_macros::Entity)]
#[entity(table = "events", crate = "vivarium_core")]
struct Event {
    id: i32,
    at: Option<DateTime<Utc>>,
    day: NaiveDate,
    trace: Uuid,
    seen: Option<Uuid>,
    blob: Vec<u8>,
    maybe_blob: Option<Vec<u8>>,
}

fn main() {
    let row = SessionRow {
        session_id: "s".into(),
        user_id: 1,
        expires_at: None,
        payload: None,
        meta: BTreeMap::new(),
    };
    assert_eq!(row.id(), "s");
    assert_eq!(SessionRow::ID_COLUMN, "session_id");

    let columns = row.columns_and_values().expect("encodes");
    let names: Vec<&str> = columns.iter().map(|(c, _)| *c).collect();
    assert_eq!(names, vec!["user_id", "expires_at", "payload", "meta"]);
    assert_eq!(
        columns[1].1,
        CoreValue::TypedNull(NullType::I64)
    );
    assert_eq!(
        columns[2].1,
        CoreValue::TypedNull(NullType::Json)
    );

    let ticket = Ticket {
        id: 3,
        kind: "bug".into(),
        cached: true,
    };
    assert_eq!(ticket.id().into_value().expect("in range"), CoreValue::I64(3));

    let event = Event {
        id: 9,
        at: None,
        day: NaiveDate::from_ymd_opt(2026, 9, 10).expect("valid date"),
        trace: Uuid::nil(),
        seen: None,
        blob: vec![1, 2, 3],
        maybe_blob: None,
    };
    assert_eq!(event.id(), 9);
    let columns = event.columns_and_values().expect("encodes");
    assert_eq!(columns.len(), 6);
    assert_eq!(columns[0].1, CoreValue::TypedNull(NullType::DateTime));
    assert_eq!(
        columns[1].1,
        CoreValue::NaiveDate(NaiveDate::from_ymd_opt(2026, 9, 10).expect("valid date"))
    );
    assert_eq!(columns[2].1, CoreValue::Uuid(Uuid::nil()));
    assert_eq!(columns[3].1, CoreValue::TypedNull(NullType::Uuid));
    assert_eq!(columns[4].1, CoreValue::Bytes(vec![1, 2, 3]));
    assert_eq!(columns[5].1, CoreValue::TypedNull(NullType::Bytes));
}
