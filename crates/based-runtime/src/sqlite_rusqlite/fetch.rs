use super::run::run;
use super::worker::Worker;
use crate::run::RowStream;
use crate::value::SqlValue;

/// Rows buffered ahead of the consumer before the worker waits.
const ROW_BUFFER: usize = 50;

/// Stream a query's rows from a worker. The query starts on first poll; dropping the
/// stream stops the worker at its next row.
pub(super) fn fetch<'a>(worker: &'a Worker, sql: &str, params: &[SqlValue]) -> RowStream<'a> {
    let (sql, params) = (sql.to_owned(), params.to_vec());
    Box::pin(async_stream::stream! {
        let (tx, mut rx) = tokio::sync::mpsc::channel(ROW_BUFFER);
        let job = move |conn: &mut rusqlite::Connection| {
            let sent = |row| tx.blocking_send(Ok(row)).is_ok();
            if let Err(e) = run(conn, &sql, &params, sent) {
                let _ = tx.blocking_send(Err(e));
            }
        };
        if let Err(e) = worker.submit(Box::new(job)) {
            yield Err(e);
            return;
        }
        while let Some(item) = rx.recv().await {
            yield item;
        }
    })
}
