//! Soft-delete `read|delete|restore: raw` overrides against a real SQLite engine: the
//! custom live rule decides what reads see, and delete/restore write the custom marker.

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen};
use crate::driver::SqliteBackend;
use based_sema::check;

const SCHEMA: &str = r#"
@soft_delete(deleted_at)
Order {
  id:         Id
  status:     text
  deleted_at: timestamp?
  archived:   bool
  read: raw`NOT archived`
  delete: raw`archived = TRUE`
  restore: raw`archived = FALSE`
}

shape OrderRow from Order { status }

query orders() -> OrderRow[] { list Order order (status); }
mutation remove(id: Id) -> OrderRow { delete Order where (id = $id); }
mutation unremove(id: Id) -> OrderRow { restore Order where (id = $id); }
"#;

// `c-stamped` has a timestamp but is not archived: live under the override, hidden by
// the default `deleted_at IS NULL` rule.
const SEED: &str = r#"
INSERT INTO `order` (`id`, `status`, `deleted_at`, `archived`) VALUES ('o1', 'a-live', NULL, 0);
INSERT INTO `order` (`id`, `status`, `deleted_at`, `archived`) VALUES ('o2', 'b-archived', NULL, 1);
INSERT INTO `order` (`id`, `status`, `deleted_at`, `archived`) VALUES ('o3', 'c-stamped', '2026-01-01 00:00:00', 0);
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

async fn live(c: &Compiled, backend: &SqliteBackend) -> serde_json::Value {
    let res = call(c, backend, "/q/orders", json!({})).await;
    assert_eq!(res.status, 200, "{:?}", res.body);
    res.body
}

#[tokio::test]
async fn overrides_drive_reads_deletes_and_restores() {
    let (c, backend) = backend().await;
    assert_eq!(
        live(&c, &backend).await,
        json!([{ "status": "a-live" }, { "status": "c-stamped" }])
    );

    let removed = call(&c, &backend, "/m/remove", json!({ "id": "o1" })).await;
    assert_eq!(removed.status, 200, "{:?}", removed.body);
    assert_eq!(removed.body, json!({ "status": "a-live" }));
    assert_eq!(live(&c, &backend).await, json!([{ "status": "c-stamped" }]));

    let restored = call(&c, &backend, "/m/unremove", json!({ "id": "o1" })).await;
    assert_eq!(restored.status, 200, "{:?}", restored.body);
    assert_eq!(restored.body, json!({ "status": "a-live" }));
    assert_eq!(
        live(&c, &backend).await,
        json!([{ "status": "a-live" }, { "status": "c-stamped" }])
    );
}
