//! Conditional writes retain their precondition but read results by a stable key.

use based_ast::FileId;
use based_codegen::{sql, Dialect};

fn lower(body: &str, dialect: Dialect) -> sql::LoweredMutation {
    let source = format!(
        r#"
        enum Status {{ resolved, closed }}
        Ticket {{ id: Id, status: Status, @index status }}
        shape TicketRow from Ticket {{ id, status }}
        mutation close_ticket(id: Id) -> TicketRow {{ {body} }}
    "#
    );
    let parsed = based_parser::parse_file(&source, FileId(0)).unwrap();
    let (schema, diagnostics) = based_sema::check(&parsed.decls);
    assert!(!diagnostics
        .iter()
        .any(|d| d.severity == based_diagnostics::Severity::Error));
    sql::lower_mutations(&schema, &parsed.decls, dialect).remove(0)
}

#[test]
fn keyed_transition_reads_back_after_its_precondition_changes() {
    for dialect in [
        Dialect::MariaDb,
        Dialect::MySql,
        Dialect::Postgres,
        Dialect::Sqlite,
    ] {
        let mutation = lower(
            "update Ticket where (id = $id and status = resolved) { status = closed };",
            dialect,
        );
        assert_eq!(mutation.match_check, Some(0));
        assert!(mutation.stmts[0].sql.contains("'resolved'"));
        assert!(mutation.stmts[0].sql.contains("'closed'"));
        let read = mutation.ret_select.unwrap();
        assert!(read.contains(":id"));
        assert!(!read.contains("'resolved'"));
    }
}

#[test]
fn a_key_under_or_does_not_identify_a_single_transition() {
    let mutation = lower(
        "update Ticket where (id = $id or status = resolved) { status = closed };",
        Dialect::Postgres,
    );
    assert_eq!(mutation.match_check, None);
    assert!(mutation.ret_select.unwrap().contains("'resolved'"));
}

#[test]
fn ordinary_keyed_updates_keep_their_readback() {
    let mutation = lower(
        "update Ticket where (id = $id) { status = closed };",
        Dialect::Postgres,
    );
    assert_eq!(mutation.match_check, None);
    assert!(mutation.ret_select.unwrap().contains(":id"));
}
