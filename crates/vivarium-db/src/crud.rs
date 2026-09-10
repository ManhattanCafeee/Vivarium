//! Generic CRUD over [`Entity`] types: find, create, update, delete, count,
//! and exists. No SQL strings anywhere — table and column names come from the
//! [`Entity`] impl (usually derived, quoted with the driver's syntax), and
//! bind values come from the closed [`Value`](vivarium_core::Value) enum.
//!
//! Primary keys are typed: every helper takes (or returns) the entity's
//! `Entity::Id`, which must implement `PrimaryKey`. A key that cannot be
//! represented as a bind value (a `u64` above `i64::MAX`, for instance) is an
//! error rather than a silent wrap.
//!
//! [`Entity`]: vivarium_core::Entity

use sqlx::Executor;
use vivarium_core::{Entity, PrimaryKey};

use crate::{DriverOps, Error, Step, encode_error, key_error};

/// Builds a bind step for a primary key.
fn id_step(id: impl PrimaryKey) -> Result<Step, Error> {
    Ok(Step::Bind(id.into_value().map_err(key_error)?))
}

/// Fetches the row with the given id, if present.
///
/// The executor may be a pool reference (`&Pool<DB>`) or a connection
/// (`&mut Connection<DB>`).
pub async fn find_by_id<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    id: T::Id,
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
    DB::fetch_optional(prefix, vec![id_step(id)?], String::new(), db).await
}

/// Inserts the entity and returns its primary key.
///
/// When `entity.id()` is set (`PrimaryKey::is_unset` is false) that key is
/// inserted as given and returned; when it is unset the id column is omitted
/// and the database assigns it (autoincrement / serial), and the generated
/// value is converted back into `T::Id` through
/// [`PrimaryKey::from_generated`].
pub async fn create<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    entity: T,
) -> Result<T::Id, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let provided = if entity.id().is_unset() {
        None
    } else {
        Some(entity.id())
    };
    if provided.is_none() && !T::Id::is_database_generated() {
        // Checked before the statement runs: an unset key that the database
        // cannot assign must not leave a half-inserted row behind.
        return Err(key_error(vivarium_core::PrimaryKeyError::new(
            "primary key is unset and this key type is not generated \
             by the database; set it explicitly",
        )));
    }

    let mut pairs = entity.columns_and_values().map_err(encode_error)?;
    if let Some(id) = &provided {
        let value = id.clone().into_value().map_err(key_error)?;
        pairs.insert(0, (T::ID_COLUMN, value));
    }

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

    match provided {
        // The key is already known, so nothing is read back: `RETURNING` would
        // decode the id column as `i64` and reject a `String` key (PostgreSQL),
        // and the driver's last-insert id is uninteresting for an explicit key.
        Some(id) => {
            DB::execute(prefix, clauses, ")".to_owned(), db).await?;
            Ok(id)
        }
        None => {
            let suffix = format!("){}", DB::returning_clause(T::ID_COLUMN));
            let generated = DB::generated_id(prefix, clauses, suffix, db).await?;
            T::Id::from_generated(generated).map_err(key_error)
        }
    }
}

/// Updates the row with the given id, setting every non-id column of
/// `entity`; the id column itself is immutable (it selects the row).
///
/// This writes **all** non-id columns, so any field left at its default is
/// written as such. Use [`Update`](crate::Update) to touch a subset of
/// columns.
///
/// Returns the number of affected rows (`0` when no such row exists).
pub async fn update_by_id<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    id: T::Id,
    entity: T,
) -> Result<u64, Error>
where
    DB: DriverOps,
    T: Entity,
{
    let pairs = entity.columns_and_values().map_err(encode_error)?;
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
    clauses.push(id_step(id)?);
    let prefix = format!("UPDATE {} SET ", DB::quote_ident(T::TABLE));
    DB::execute(prefix, clauses, String::new(), db).await
}

/// Deletes the row with the given id.
///
/// Returns the number of affected rows (`0` when no such row existed).
pub async fn delete<'q, T, DB>(
    db: impl Executor<'q, Database = DB> + 'q,
    id: T::Id,
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
    DB::execute(prefix, vec![id_step(id)?], String::new(), db).await
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
    id: T::Id,
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
    DB::scalar_bool(prefix, vec![id_step(id)?], suffix, db).await
}
