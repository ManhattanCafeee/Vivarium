//! Connection-pool conveniences.

use log::LevelFilter;
use sqlx::ConnectOptions;
use std::time::Duration;

/// Enables query logging on a [`ConnectOptions`]: statements at `TRACE` and
/// slow statements (> 1 s) at `WARN`.
///
/// ```
/// # #[cfg(feature = "sqlite")] {
/// # use vivarium_db::pool::with_query_logging;
/// use sqlx::sqlite::SqliteConnectOptions;
///
/// let options = with_query_logging(SqliteConnectOptions::new());
/// # let _ = options;
/// # }
/// ```
pub fn with_query_logging<Opt: ConnectOptions>(options: Opt) -> Opt {
    options
        .log_statements(LevelFilter::Trace)
        .log_slow_statements(LevelFilter::Warn, Duration::from_secs(1))
}
