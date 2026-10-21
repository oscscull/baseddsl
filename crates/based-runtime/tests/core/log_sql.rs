//! The statement-logging backend is a transparent pass-through to the backend it wraps.

use based_runtime::run::{fetch_all, LoggedBackend};
use based_runtime::{Backend, MockDb, SqlValue};

#[tokio::test]
async fn logged_backend_runs_every_statement_on_the_inner_backend() {
    let mock = MockDb::new(vec![vec![]]);
    let logged = LoggedBackend::new(mock.clone());
    let db = logged.checkout("").await.unwrap();
    let mut tx = db.begin().await.unwrap();
    tx.execute("UPDATE t SET a = ?", &[SqlValue::Int(1)])
        .await
        .unwrap();
    fetch_all(tx.fetch("SELECT 1", &[])).await.unwrap();
    tx.commit().await.unwrap();

    let sql: Vec<_> = mock.calls().into_iter().map(|(sql, _)| sql).collect();
    assert_eq!(sql, ["UPDATE t SET a = ?", "SELECT 1"]);
    assert_eq!(mock.tx_log(), ["begin", "commit"]);
}
