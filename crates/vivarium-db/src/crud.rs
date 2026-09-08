//! Generic CRUD over [`Entity`] types: find, create, update, delete, count,
//! and exists. No SQL strings anywhere — table and column names come from the
//! [`Entity`] impl (usually derived, quoted with the driver's syntax), and
//! bind values come from the closed [`Value`] enum, bound by reference.
//!
//! [`Entity`]: vivarium_core::Entity

use sqlx::Executor;
use vivarium_core::{Entity, Value};

use crate::{DriverOps, Error, Step};

/// Builds a bind step for a single id value.
fn id_bind(id: i64) -> Step {
    Step::Bind(Value::I64(id))
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
    let prefix = format!(
        "SELECT * FROM {} WHERE {} = ",
        DB::quote_ident(T::TABLE),
        DB::quote_ident(T::ID_COLUMN)
    );
    DB::fetch_optional(prefix, vec![id_bind(id)], String::new(), db).await
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
        .map(|(col, _)| DB::quote_ident(col))
        .collect::<Vec<_>>()
        .join(", ");

    let mut clauses: Vec<Step> = Vec::new();
    for (i, (_, value)) in pairs.into_iter().enumerate() {
        if i > 0 {
            clauses.push(Step::Text(", ".to_owned()));
        }
        clauses.push(Step::Bind(value));
    }
    let prefix = format!(
        "INSERT INTO {} ({cols}) VALUES (",
        DB::quote_ident(T::TABLE)
    );
    let suffix = format!("){}", DB::returning_clause(T::ID_COLUMN));
    DB::generated_id(prefix, clauses, suffix, db).await
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
    let mut clauses: Vec<Step> = Vec::new();
    for (i, (col, value)) in pairs.into_iter().enumerate() {
        if i > 0 {
            clauses.push(Step::Text(", ".to_owned()));
        }
        clauses.push(Step::Text(format!("{} = ", DB::quote_ident(col))));
        clauses.push(Step::Bind(value));
    }
    clauses.push(Step::Text(format!(
        " WHERE {} = ",
        DB::quote_ident(T::ID_COLUMN)
    )));
    clauses.push(id_bind(id));
    let prefix = format!("UPDATE {} SET ", DB::quote_ident(T::TABLE));
    DB::execute(prefix, clauses, String::new(), db).await
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
    let prefix = format!(
        "DELETE FROM {} WHERE {} = ",
        DB::quote_ident(T::TABLE),
        DB::quote_ident(T::ID_COLUMN)
    );
    DB::execute(prefix, vec![id_bind(id)], String::new(), db).await
}

/// Counts all rows of the entity's table.
pub async fn count<'q, T, DB>(db: impl Executor<'q, Database = DB> + 'q) -> Result<i64, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let prefix = format!("SELECT COUNT(*) FROM {}", DB::quote_ident(T::TABLE));
    DB::scalar_i64(prefix, Vec::new(), String::new(), db).await
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
    let prefix = format!(
        "SELECT EXISTS(SELECT 1 FROM {} WHERE {} = ",
        DB::quote_ident(T::TABLE),
        DB::quote_ident(T::ID_COLUMN)
    );
    let suffix = ")".to_owned();
    DB::scalar_bool(prefix, vec![id_bind(id)], suffix, db).await
}
