//! Generic CRUD over [`Entity`] types: find, create, update, delete, count,
//! and exists. No SQL strings anywhere — table and column names come from the
//! [`Entity`] impl (usually derived), and bind values are type-checked per
//! driver.
//!
//! [`Entity`]: vivarium_core::Entity

use sqlx::Executor;
use std::marker::PhantomData;
use vivarium_core::{Entity, Value};

use crate::{DriverOps, Error, Step, ValueBinder};

/// Builds a bind step for a single id value.
fn id_bind<DB: DriverOps>(id: i64) -> Step<DB> {
    Step::Bind(std::sync::Arc::new(ValueBinder {
        value: Value::I64(id),
        _db: PhantomData,
    }))
}

/// Fetches the row with the given id, if present.
///
/// The executor may be a pool reference (`&Pool<DB>`) or a connection
/// (`&mut Connection<DB>`).
pub async fn find_by_id<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    id: i64,
) -> Result<Option<T>, Error>
where
    DB: DriverOps,
    T: Entity + for<'r> sqlx::FromRow<'r, DB::Row> + Send + Unpin,
{
    let steps = vec![
        Step::Text(format!(
            "SELECT * FROM {} WHERE {} = ",
            T::TABLE,
            T::ID_COLUMN
        )),
        id_bind::<DB>(id),
    ];
    DB::fetch_optional(steps, db).await
}

/// Inserts the entity and returns its id.
///
/// When `entity.id()` is non-zero the id is inserted as given; when it is
/// zero the id column is omitted and the database assigns it (autoincrement /
/// serial). The generated (or provided) id is returned.
pub async fn create<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    entity: T,
) -> Result<i64, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let pairs: Vec<(&'static str, Value)> = if entity.id() != 0 {
        std::iter::once((T::ID_COLUMN, Value::I64(entity.id())))
            .chain(entity.columns_and_values())
            .collect()
    } else {
        entity.columns_and_values()
    };
    let cols = pairs
        .iter()
        .map(|(col, _)| *col)
        .collect::<Vec<_>>()
        .join(", ");

    let mut steps: Vec<Step<DB>> = vec![Step::Text(format!(
        "INSERT INTO {} ({cols}) VALUES (",
        T::TABLE
    ))];
    for (i, (_, value)) in pairs.into_iter().enumerate() {
        if i > 0 {
            steps.push(Step::Text(", ".to_owned()));
        }
        steps.push(Step::Bind(std::sync::Arc::new(ValueBinder {
            value,
            _db: PhantomData,
        })));
    }
    steps.push(Step::Text(format!(
        "){}",
        DB::returning_clause(T::ID_COLUMN)
    )));
    DB::generated_id(steps, db).await
}

/// Updates the row with the given id, setting every non-id column of
/// `entity`; the id column itself is immutable (it selects the row).
///
/// Returns the number of affected rows (`0` when no such row exists).
pub async fn update_by_id<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    id: i64,
    entity: T,
) -> Result<u64, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let pairs = entity.columns_and_values();
    if pairs.is_empty() {
        return Err(Error::Protocol(
            "entity has no updatable columns".to_owned(),
        ));
    }
    let mut steps: Vec<Step<DB>> = vec![Step::Text(format!("UPDATE {} SET ", T::TABLE))];
    for (i, (col, value)) in pairs.into_iter().enumerate() {
        if i > 0 {
            steps.push(Step::Text(", ".to_owned()));
        }
        steps.push(Step::Text(format!("{col} = ")));
        steps.push(Step::Bind(std::sync::Arc::new(ValueBinder {
            value,
            _db: PhantomData,
        })));
    }
    steps.push(Step::Text(format!(" WHERE {} = ", T::ID_COLUMN)));
    steps.push(id_bind::<DB>(id));
    DB::execute(steps, db).await
}

/// Deletes the row with the given id.
///
/// Returns the number of affected rows (`0` when no such row existed).
pub async fn delete<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    id: i64,
) -> Result<u64, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let steps = vec![
        Step::Text(format!(
            "DELETE FROM {} WHERE {} = ",
            T::TABLE,
            T::ID_COLUMN
        )),
        id_bind::<DB>(id),
    ];
    DB::execute(steps, db).await
}

/// Counts all rows of the entity's table.
pub async fn count<'q, T, DB>(db: impl Executor<'q, Database = DB> + 'q) -> Result<i64, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let steps = vec![Step::Text(format!("SELECT COUNT(*) FROM {}", T::TABLE))];
    DB::scalar_i64(steps, db).await
}

/// Whether a row with the given id exists.
pub async fn exists<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    id: i64,
) -> Result<bool, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let steps = vec![
        Step::Text(format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE {} = ",
            T::TABLE,
            T::ID_COLUMN
        )),
        id_bind::<DB>(id),
        Step::Text(")".to_owned()),
    ];
    DB::scalar_bool(steps, db).await
}
