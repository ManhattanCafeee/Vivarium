-- Example migration shipped with vivarium-db (SQLite dialect).
-- The crate embeds no migrator on purpose: a library-owned
-- `_sqlx_migrations` table would have collided with the host application's,
-- and SQLite-flavoured DDL would pollute a MySQL or PostgreSQL database.
-- Copy this layout into your own `migrations/` directory and apply it with
-- `sqlx::migrate!("./migrations")`.
CREATE TABLE users (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    name    TEXT    NOT NULL,
    email   TEXT,
    age     INTEGER NOT NULL DEFAULT 0,
    active  INTEGER NOT NULL DEFAULT 0,
    profile TEXT
);
