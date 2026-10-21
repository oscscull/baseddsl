//! Opt-in statement logging (`BASED_LOG_SQL=1`): every statement a backend executes is
//! written to stderr with its bind count. Bound values are never logged.

mod backend;
mod connection;
mod line;

use super::Backend;
pub use backend::LoggedBackend;
pub use connection::Logged;

/// The environment variable that turns statement logging on.
pub const LOG_SQL_ENV: &str = "BASED_LOG_SQL";

/// Whether `BASED_LOG_SQL` is set to anything but empty, `0`, or `false`.
pub fn log_sql_enabled() -> bool {
    std::env::var(LOG_SQL_ENV).is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false"))
}

/// `backend`, logging every statement when `BASED_LOG_SQL` is on.
pub fn with_statement_log(backend: impl Backend + 'static) -> Box<dyn Backend> {
    match log_sql_enabled() {
        true => Box::new(LoggedBackend::new(backend)),
        false => Box::new(backend),
    }
}
