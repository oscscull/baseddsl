//! Tests for the derived-fact computation: parse a snippet, check it, and assert
//! on the facts an editor would surface (principle 8).

use based_ast::FileId;
use based_facts::{facts, Fact, FactKind};
use based_parser::parse_file;
use based_sema::check;

fn facts_of(src: &str) -> Vec<Fact> {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
    facts(&schema, &sf.decls)
}

fn of_kind(fs: &[Fact], kind: FactKind) -> Vec<&Fact> {
    fs.iter().filter(|f| f.kind == kind).collect()
}

// The canonical case: an inverse edge (`items: OrderItem[]`) whose forward pair is
// inferred, and the join-key index that traversal forces on the FK-holding model.
const TRAVERSAL: &str = r#"
    @sort(placed_at desc)
    Order { placed_at: timestamp, items: OrderItem[], @index placed_at }
    OrderItem { order: Order, qty: int }
    shape O from Order { first_qty = items.qty }
    query orders() -> O[];
"#;

#[test]
fn inferred_inverse_is_shown() {
    let fs = facts_of(TRAVERSAL);
    let inv = of_kind(&fs, FactKind::InferredInverse);
    assert_eq!(inv.len(), 1, "{fs:#?}");
    assert_eq!(inv[0].label, "<- OrderItem via order");
    assert!(
        inv[0].detail.contains("OrderItem.order"),
        "{}",
        inv[0].detail
    );
}

#[test]
fn inferred_index_matches_ddl_naming() {
    let fs = facts_of(TRAVERSAL);
    let idx = of_kind(&fs, FactKind::InferredIndex);
    assert_eq!(idx.len(), 1, "{fs:#?}");
    // No soft-delete on OrderItem; FK field `order` -> physical `order_id`.
    assert_eq!(idx[0].label, "index inf_order_item_order (order_id)");
}

#[test]
fn explicit_inverse_pairing_is_not_shown() {
    // Author wrote `(OrderItem.order)` — the pairing is in source, so it is not a
    // "show, don't write" fact.
    let fs = facts_of(
        r#"
        Order { placed_at: timestamp, items: OrderItem[] (OrderItem.order) }
        OrderItem { order: Order, qty: int }
        shape O from Order { placed_at }
        query orders() -> O[] order (placed_at);
    "#,
    );
    assert!(
        of_kind(&fs, FactKind::InferredInverse).is_empty(),
        "{fs:#?}"
    );
}

#[test]
fn soft_delete_column_leads_the_inferred_index() {
    // A `@soft_delete` model prepends its tombstone column to the inferred key
    // (predicate-leading — MariaDB has no partial indexes), matching `sql::ddl`.
    let fs = facts_of(
        r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, placed_at: timestamp, items: OrderItem[], @index placed_at }
        @soft_delete(deleted_at)
        OrderItem { deleted_at: timestamp?, order: Order, qty: int }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[] order (placed_at);
    "#,
    );
    let idx = of_kind(&fs, FactKind::InferredIndex);
    assert_eq!(idx.len(), 1, "{fs:#?}");
    assert_eq!(
        idx[0].label,
        "index inf_order_item_deleted_at_order (deleted_at, order_id)"
    );
}

#[test]
fn no_derived_facts_on_a_flat_schema() {
    // No relations, no traversal -> nothing to show.
    let fs = facts_of(
        r#"
        Product { name: text, @index name }
        shape P from Product { name }
        query products() -> P[] order (name);
    "#,
    );
    assert!(fs.is_empty(), "{fs:#?}");
}
