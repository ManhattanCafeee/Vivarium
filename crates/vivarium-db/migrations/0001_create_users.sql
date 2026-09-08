-- Example migration shipped with vivarium-db (SQLite dialect).
-- Used by the crate's own test suite; your application ships its own
-- migrations and applies vivarium_db::MIGRATOR or its own migrator.
CREATE TABLE users (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    name    TEXT    NOT NULL,
    email   TEXT,
    age     INTEGER NOT NULL DEFAULT 0,
    active  INTEGER NOT NULL DEFAULT 0,
    profile TEXT
);
