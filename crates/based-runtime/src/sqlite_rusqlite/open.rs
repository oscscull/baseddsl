use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

use super::error::map_err;
use super::worker::Opener;
use crate::run::DbError;

/// How long a locked database waits before `SQLITE_BUSY` (a retryable deadlock).
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Fresh `:memory:` connections with foreign keys enforced.
pub(super) fn memory() -> Opener {
    Arc::new(|| {
        let conn = Connection::open_in_memory().map_err(map_err)?;
        base(&conn)?;
        Ok(conn)
    })
}

/// Connections to the file at `path` (created if absent) in WAL mode with `NORMAL`
/// sync: readers run alongside a writer, and commits survive an application crash.
pub(super) fn file(path: &str) -> Opener {
    let path = path.to_owned();
    Arc::new(move || {
        let conn = Connection::open(&path).map_err(map_err)?;
        base(&conn)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
            .map_err(map_err)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(map_err)?;
        Ok(conn)
    })
}

/// The caller's own connection, used as configured; it can be opened only once.
pub(super) fn given(conn: Connection) -> Opener {
    let slot = Mutex::new(Some(conn));
    Arc::new(move || {
        slot.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| DbError::new("the provided sqlite connection is no longer available"))
    })
}

/// Settings every engine-opened connection gets: foreign keys on (SQLite ignores them
/// otherwise, so opt-in `@fk` actions would not enforce) and a bounded busy wait.
fn base(conn: &Connection) -> Result<(), DbError> {
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(map_err)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(map_err)
}
