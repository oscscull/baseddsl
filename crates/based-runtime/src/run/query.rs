use super::*;

/// Collect a [`RowStream`] into a `Vec` — the one-shot read path.
pub async fn fetch_all(stream: RowStream<'_>) -> Result<Vec<Row>, DbError> {
    use futures_util::TryStreamExt;
    stream.try_collect().await
}

/// Plan and run a query request, returning the shaped JSON response. Takes any
/// [`DbRead`] — a checked-out connection or an open transaction (generic so a
/// `&mut dyn Db` / `&mut dyn Tx` passes straight in).
pub async fn run_query<D: DbRead + ?Sized>(
    compiled: &Compiled,
    db: &mut D,
    req: &Request,
) -> Result<serde_json::Value, RunError> {
    let plan = plan_query(compiled, req)?;
    Ok(shape(db, &plan).await?)
}

/// Plan a query request and return its rows as an owned [`ShapedStream`] — the
/// `-> stream` read path. Planning (arg / `$ctx` validation) happens before the first
/// row, so a boundary failure is an ordinary [`PlanError`] and the stream never
/// starts. The same plan → fetch → shape path as [`run_query`], minus the collect:
/// scope, soft-delete, and shaping are identical to the `[]` form.
///
/// Takes the connection by value: the returned stream owns it for the whole pass, and
/// dropping the stream (caller cancelled) drops the connection back to the pool.
pub fn run_query_stream(
    compiled: &Compiled,
    mut db: Box<dyn Db>,
    req: &Request,
) -> Result<ShapedStream, PlanError> {
    use futures_util::StreamExt;
    let plan = plan_query(compiled, req)?;
    Ok(Box::pin(async_stream::stream! {
        let mut rows = db.fetch(&plan.main.sql, &plan.main.params);
        while let Some(item) = rows.next().await {
            match item {
                Ok(row) => {
                    let mut v = nest_row(row);
                    if !plan.json_paths.is_empty() {
                        normalize_json(&mut v, &plan.json_paths);
                    }
                    yield Ok(v);
                }
                // A mid-stream failure is the stream's last item.
                Err(e) => {
                    yield Err(e);
                    return;
                }
            }
        }
    }))
}

/// Run one statement to completion — the collected one-shot read, for callers holding
/// a [`Stmt`].
pub async fn run_stmt<D: DbRead + ?Sized>(db: &mut D, stmt: &Stmt) -> Result<Vec<Row>, DbError> {
    fetch_all(db.fetch(&stmt.sql, &stmt.params)).await
}

