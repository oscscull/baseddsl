//! Real conditional-write/read-back behavior, including scope and no-op failures.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_runtime::{Compiled, Engine, SeqIdGen, SqliteBackend};
use serde_json::json;

#[tokio::test]
async fn conditional_transition_returns_the_written_row_or_not_found() {
    let parsed = based_parser::parse_file(
        r#"
        scope Tenant (tenant: text = $ctx.tenant)
        enum Status { resolved, closed }
        @scope Tenant
        Ticket { id: Id, tenant: text, status: Status }
        shape TicketRow from Ticket { id, status }
        mutation close_ticket(id: Id) -> TicketRow scoped Tenant {
            update Ticket where (id = $id and status = resolved) { status = closed };
        }
    "#,
        FileId(0),
    )
    .unwrap();
    let (schema, diagnostics) = based_sema::check(&parsed.decls);
    assert!(!diagnostics
        .iter()
        .any(|d| d.severity == based_diagnostics::Severity::Error));
    let backend = SqliteBackend::in_memory().unwrap();
    backend
        .execute_batch(&sql::ddl(&schema, Dialect::Sqlite))
        .await
        .unwrap();
    backend.execute_batch("INSERT INTO ticket (id, tenant, status) VALUES ('a', 'mine', 'resolved'), ('b', 'other', 'resolved');")
        .await.unwrap();
    let engine = Engine::new(
        Compiled::from_checked(schema, parsed.decls, Dialect::Sqlite),
        backend,
        SeqIdGen::default(),
    );
    let closed = engine
        .call(
            "/m/close_ticket",
            json!({"id": "a"}),
            json!({"tenant": "mine"}),
        )
        .await;
    assert_eq!(closed.status, 200, "{:?}", closed.body);
    assert_eq!(closed.body["status"], "closed");
    for id in ["a", "b", "missing"] {
        let response = engine
            .call(
                "/m/close_ticket",
                json!({"id": id}),
                json!({"tenant": "mine"}),
            )
            .await;
        assert_eq!(response.status, 404, "{:?}", response.body);
        assert_eq!(response.body["error"]["code"], "not_found");
    }
    let other = engine
        .call(
            "/m/close_ticket",
            json!({"id": "b"}),
            json!({"tenant": "other"}),
        )
        .await;
    assert_eq!(
        other.status, 200,
        "cross-tenant failed write left it resolved: {:?}",
        other.body
    );
}
