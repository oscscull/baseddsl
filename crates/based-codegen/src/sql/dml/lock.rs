//! Row locking: `for update` / lock-wait clause emission.

use super::*;

/// `get|list … for update` — a pessimistic locking read (`SELECT … FOR UPDATE`, a no-op on
/// SQLite via the [`Dialect`](crate::Dialect) seam). Block-body only. Sema confines it to
/// well-defined single-row sets; the client confines it to transaction transports.
fn query_for_update(q: &Query) -> Option<LockWait> {
    match &q.body {
        QueryBody::Block(s) => s.for_update,
        _ => None,
    }
}

/// Append the `for update` row-locking clause (after ORDER BY/LIMIT) per dialect: `FOR UPDATE`
/// (plus its optional `NOWAIT`/`SKIP LOCKED` wait mode) on Postgres/MySQL/MariaDB, nothing on
/// SQLite (its transaction lock already serializes writers).
pub(crate) fn push_lock_clause(sql: &mut String, q: &Query, dialect: Dialect) {
    if let Some(wait) = query_for_update(q) {
        let lock = dialect.for_update_clause(wait);
        if !lock.is_empty() {
            sql.push_str(&format!("\n{lock}"));
        }
    }
}
