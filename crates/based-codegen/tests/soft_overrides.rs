//! Soft-delete `read|delete|restore: raw` overrides: each replaces the default SQL at
//! every site that op lowers to, on every dialect.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

const SCHEMA: &str = r#"
    Customer { id: Id, name: text, orders: Order[] }
    @soft_delete(deleted_at)
    @updated(updated_at)
    Order {
      id: Id
      customer: Customer
      status: text
      deleted_at: timestamp?
      updated_at: timestamp
      archived: bool
      read: raw`NOT archived AND {id} > 0`
      delete: raw`deleted_at = CURRENT_TIMESTAMP, archived = true`
      restore: raw`deleted_at = NULL, archived = false`
    }
    Item { id: Id, order: Order, sku: text }
    shape OrderRow from Order { status }
    shape ItemRow from Item { sku, ostatus = order.status }
    shape CustRow from Customer { name, orders { status } }
    query orders() -> OrderRow[] { list Order order (status); }
    query items() -> ItemRow[] { list Item order (sku); }
    query custs() -> CustRow[] { list Customer order (name); }
    query busy() -> CustRow[] { list Customer where (orders.any()) order (name); }
    mutation remove(id: Id) -> OrderRow { delete Order where (id = $id); }
    mutation unremove(id: Id) -> OrderRow { restore Order where (id = $id); }
    mutation purge(id: Id) -> ok { hard delete Order where (id = $id); }
"#;

const DIALECTS: [Dialect; 4] = [
    Dialect::MariaDb,
    Dialect::MySql,
    Dialect::Postgres,
    Dialect::Sqlite,
];

fn lower(dialect: Dialect) -> (String, String) {
    let sf = parse_file(SCHEMA, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    (
        sql::dml::dml(&schema, &sf.decls, dialect),
        sql::mutations::mutations(&schema, &sf.decls, dialect),
    )
}

/// The section of generated output headed by `-- <kind> <name>`.
fn section<'a>(out: &'a str, kind: &str, name: &str) -> &'a str {
    let head = format!("-- {kind} {name}\n");
    let start = out
        .find(&head)
        .unwrap_or_else(|| panic!("no {head:?} in\n{out}"));
    let rest = &out[start..];
    let next = format!("\n-- {kind} ");
    match rest[head.len()..].find(&next) {
        Some(i) => &rest[..head.len() + i],
        None => rest,
    }
}

#[test]
fn read_override_replaces_the_live_filter_at_every_read_site() {
    let (dml, _) = lower(Dialect::MariaDb);
    assert!(
        section(&dml, "query", "orders").contains("WHERE (NOT archived AND `order`.`id` > 0)"),
        "\n{dml}"
    );
    assert!(
        section(&dml, "query", "items").contains("AND (NOT archived AND `j_order`.`id` > 0)"),
        "\n{dml}"
    );
    assert!(
        section(&dml, "query", "custs").contains("AND (NOT archived AND `s1_order`.`id` > 0)"),
        "\n{dml}"
    );
    let busy = section(&dml, "query", "busy");
    assert!(
        busy.contains("(NOT archived AND `s") && busy.contains("_order`.`id` > 0)"),
        "\n{busy}"
    );
    assert!(
        !dml.contains("`deleted_at` IS NULL"),
        "default filter leaked:\n{dml}"
    );
}

#[test]
fn delete_override_replaces_the_tombstone_write_and_keeps_the_guards() {
    let (_, muts) = lower(Dialect::MariaDb);
    let remove = section(&muts, "mutation", "remove");
    assert!(
        remove.contains(
            "UPDATE `order`\nSET deleted_at = CURRENT_TIMESTAMP, archived = true, `order`.`updated_at` = CURRENT_TIMESTAMP\nWHERE `order`.`id` = :id AND (NOT archived AND `order`.`id` > 0);"
        ),
        "\n{remove}"
    );
}

#[test]
fn restore_override_replaces_the_restore_write() {
    let (_, muts) = lower(Dialect::MariaDb);
    let restore = section(&muts, "mutation", "unremove");
    assert!(
        restore.contains(
            "UPDATE `order`\nSET deleted_at = NULL, archived = false, `order`.`updated_at` = CURRENT_TIMESTAMP\nWHERE `order`.`id` = :id;"
        ),
        "\n{restore}"
    );
}

#[test]
fn hard_delete_ignores_the_overrides() {
    let (_, muts) = lower(Dialect::MariaDb);
    let purge = section(&muts, "mutation", "purge");
    assert!(
        purge.contains("DELETE FROM `order`\nWHERE `order`.`id` = :id;"),
        "\n{purge}"
    );
    assert!(!purge.contains("archived"), "\n{purge}");
}

#[test]
fn overrides_apply_on_every_dialect() {
    for dialect in DIALECTS {
        let (dml, muts) = lower(dialect);
        let q = dialect.quote("order");
        let live = format!("(NOT archived AND {q}.{} > 0)", dialect.quote("id"));
        assert!(
            section(&dml, "query", "orders").contains(&live),
            "{dialect:?}\n{dml}"
        );
        assert!(
            !dml.contains("IS NULL"),
            "{dialect:?} default filter leaked:\n{dml}"
        );
        let remove = section(&muts, "mutation", "remove");
        assert!(
            remove.contains("SET deleted_at = CURRENT_TIMESTAMP, archived = true"),
            "{dialect:?}\n{remove}"
        );
        assert!(remove.contains(&live), "{dialect:?}\n{remove}");
        let restore = section(&muts, "mutation", "unremove");
        assert!(
            restore.contains("SET deleted_at = NULL, archived = false"),
            "{dialect:?}\n{restore}"
        );
    }
}
