//! End-to-end proof of the `serial` (DB-generated primary key) read-back path against a
//! **real** SQLite engine — no mock.
//!
//! A `serial` model's INSERT omits the id column; the database assigns it and the engine
//! reads it back (`RETURNING id` on SQLite) to key the declared-shape re-select. This test
//! runs the verbatim `based gen sql` DDL + the runtime's lowered DML against a live
//! in-memory SQLite database, so a passing run means the whole serial path — omit-id
//! INSERT, DB-assigned id, read-back, integer wire id — actually executes.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
Org { id: serial, name: text }
Ticket { id: serial, org: Org, title: text, @index org }

shape OrgCard from Org { id, name }
shape TicketCard from Ticket { id, title, org = org.id }

mutation create_org(name) -> OrgCard { create Org { name = $name }; }
mutation create_ticket(org -> org, title) -> TicketCard {
  create Ticket { org = $org, title = $title };
}
# A model-annotated param referencing a serial-keyed model: the wire value is that
# model's integer key, not the project-default uuid (regression: this coerced as uuid
# server-side and rejected the number).
mutation delete_org_tickets(org: Org) -> ok { delete Ticket where (org = $org); }
query org_by_id(id) -> OrgCard;
query ticket_by_id(id) -> TicketCard;
"#;

/// Compile the inline serial schema (`serial` is written explicitly, so no manifest
/// default resolution is needed) and seed an in-memory SQLite database with the generated
/// SQLite DDL — the same artifact `based gen sql` emits.
async fn serial_backend() -> (Compiled, SqliteBackend) {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean: {diags:?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("serial DDL failed to execute: {e:?}\n{ddl}"));
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
async fn serial_create_reads_back_the_db_generated_id() {
    // The create omits the id; SQLite assigns it; the engine reads it back into the
    // declared shape. The wire id is a JSON *number*, and successive creates increment it.
    let (c, backend) = serial_backend().await;

    let first = call(&c, &backend, "/m/create_org", json!({ "name": "Acme" })).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body["name"], json!("Acme"));
    let id1 = first.body["id"].as_i64().expect("id is an integer");

    let second = call(&c, &backend, "/m/create_org", json!({ "name": "Globex" })).await;
    assert_eq!(second.status, 200, "{:?}", second.body);
    let id2 = second.body["id"].as_i64().expect("id is an integer");
    assert!(id2 > id1, "serial ids increment: {id1} then {id2}");

    // Get-by-id keys on the DB-generated integer id and returns the same row.
    let got = call(&c, &backend, "/q/org_by_id", json!({ "id": id1 })).await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body, json!({ "id": id1, "name": "Acme" }));
}

#[tokio::test]
async fn serial_fk_references_a_db_generated_parent() {
    // A relation FK to a serial model is a plain integer column: create an org, then a
    // ticket whose `org` FK is that org's DB-generated id (passed as the request arg).
    let (c, backend) = serial_backend().await;

    let org = call(&c, &backend, "/m/create_org", json!({ "name": "Acme" })).await;
    let org_id = org.body["id"].as_i64().expect("org id");

    let ticket = call(
        &c,
        &backend,
        "/m/create_ticket",
        json!({ "org": org_id, "title": "Broken login" }),
    )
    .await;
    assert_eq!(ticket.status, 200, "{:?}", ticket.body);
    let ticket_id = ticket.body["id"].as_i64().expect("ticket id");
    assert_eq!(ticket.body["title"], json!("Broken login"));
    assert_eq!(ticket.body["org"], json!(org_id));

    let got = call(&c, &backend, "/q/ticket_by_id", json!({ "id": ticket_id })).await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(
        got.body,
        json!({ "id": ticket_id, "title": "Broken login", "org": org_id })
    );
}

#[tokio::test]
async fn model_annotated_serial_fk_param_accepts_the_integer_key() {
    // Regression: a `org: Org` param references a serial-keyed model, so its wire value is
    // an integer. The server-side coercion used to default a model-annotated param to uuid
    // and reject the number (`400 bad_arg: expected uuid, got number`). It must accept the
    // integer id and delete the matching rows.
    let (c, backend) = serial_backend().await;

    let org = call(&c, &backend, "/m/create_org", json!({ "name": "Acme" })).await;
    let org_id = org.body["id"].as_i64().expect("org id");
    for title in ["a", "b"] {
        let t = call(
            &c,
            &backend,
            "/m/create_ticket",
            json!({ "org": org_id, "title": title }),
        )
        .await;
        assert_eq!(t.status, 200, "{:?}", t.body);
    }

    // The serial FK arrives as a JSON number — accepted, not rejected as a uuid.
    let deleted = call(
        &c,
        &backend,
        "/m/delete_org_tickets",
        json!({ "org": org_id }),
    )
    .await;
    assert_eq!(deleted.status, 200, "{:?}", deleted.body);

    // A stringy uuid for the same param is a type error, proving it coerces as an integer.
    let bad = call(
        &c,
        &backend,
        "/m/delete_org_tickets",
        json!({ "org": "3f2504e0-4f89-41d3-9a0c-0305e82c3301" }),
    )
    .await;
    assert_eq!(
        bad.status, 400,
        "a uuid string is not a serial key: {:?}",
        bad.body
    );
}
