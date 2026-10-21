use super::run::run;
use super::worker::Worker;
use crate::run::DbError;
use crate::value::SqlValue;

/// Run a write (or any statement batch) on a worker; returns rows affected.
pub(super) async fn execute(
    worker: &Worker,
    sql: &str,
    params: &[SqlValue],
) -> Result<u64, DbError> {
    let (sql, params) = (sql.to_owned(), params.to_vec());
    worker
        .call(move |conn| run(conn, &sql, &params, |_| true))
        .await?
}
