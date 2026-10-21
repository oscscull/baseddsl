//! A named filter projected as a bool shape field, against a real SQLite engine: the
//! filter's `$before` binds from the query's param, and a row the filter evaluates to
//! NULL for (a NULL `since`) projects false, not null.

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen};
use crate::driver::SqliteBackend;
use based_sema::check;

const SCHEMA: &str = r#"
Task {
  id:     Id
  title:  text
  status: text
  since:  timestamp?
}

filter overdue(before) = status = "waiting" and since <= $before;

shape TaskCard from Task {
  title
  is_overdue = overdue($before)
}

query board(before: timestamp) -> TaskCard[] { list Task order (title); }
query due(before: timestamp) -> TaskCard[] { list Task where (overdue($before)) order (title); }
"#;

const SEED: &str = r#"
INSERT INTO `task` (`id`, `title`, `status`, `since`) VALUES ('t1', 'a-old', 'waiting', '2026-01-01 00:00:00');
INSERT INTO `task` (`id`, `title`, `status`, `since`) VALUES ('t2', 'b-new', 'waiting', '2026-06-01 00:00:00');
INSERT INTO `task` (`id`, `title`, `status`, `since`) VALUES ('t3', 'c-done', 'done', '2026-01-01 00:00:00');
INSERT INTO `task` (`id`, `title`, `status`, `since`) VALUES ('t4', 'd-unset', 'waiting', NULL);
"#;

async fn backend() -> (Compiled, SqliteBackend) {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .collect();
    assert!(errs.is_empty(), "schema should check clean: {errs:?}");
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed to execute: {e:?}\n{ddl}"));
    backend.execute_batch(SEED).await.expect("seed");
    (compiled, backend)
}

async fn call(
    compiled: &Compiled,
    backend: &SqliteBackend,
    path: &str,
    args: serde_json::Value,
) -> based_runtime::WireResponse {
    let ids = SeqIdGen::default();
    dispatch(
        compiled,
        backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        path,
        args,
        json!({}),
        None,
    )
    .await
}

#[tokio::test]
async fn filter_projection_explains_each_row() {
    let (c, backend) = backend().await;
    let before = json!({ "before": "2026-03-01 00:00:00" });

    // SQLite carries a bool as 0/1, like any bool column; the client decodes it.
    let board = call(&c, &backend, "/q/board", before.clone()).await;
    assert_eq!(board.status, 200, "{:?}", board.body);
    assert_eq!(
        board.body,
        json!([
            { "title": "a-old", "is_overdue": 1 },
            { "title": "b-new", "is_overdue": 0 },
            { "title": "c-done", "is_overdue": 0 },
            { "title": "d-unset", "is_overdue": 0 },
        ])
    );

    // The same filter selects and explains: every matched row projects true.
    let due = call(&c, &backend, "/q/due", before).await;
    assert_eq!(due.status, 200, "{:?}", due.body);
    assert_eq!(due.body, json!([{ "title": "a-old", "is_overdue": 1 }]));
}
