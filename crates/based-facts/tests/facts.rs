//! Tests for the derived-fact computation: parse a snippet, check it, and assert on
//! the facts an editor would surface.

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
    // The forward edge it pairs through, model-qualified so `order` (a field name that
    // echoes a model) is unambiguous; it is also the command-click target in `nav`.
    assert_eq!(inv[0].label, "via OrderItem.order");
    assert!(
        inv[0].detail.contains("OrderItem.order"),
        "{}",
        inv[0].detail
    );
    // `nav` points at the paired forward edge `OrderItem.order` so the inlay is
    // followable (the LSP renders it as a clickable label part).
    assert!(inv[0].nav.is_some(), "inverse fact carries a nav target");
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
    // No relations, no traversal -> no inverse/index facts. A query still resolves a
    // shape, so that one fact remains.
    let fs = facts_of(
        r#"
        Product { name: text, @index name }
        shape P from Product { name }
        query products() -> P[] order (name);
    "#,
    );
    assert!(
        of_kind(&fs, FactKind::InferredInverse).is_empty(),
        "{fs:#?}"
    );
    assert!(of_kind(&fs, FactKind::InferredIndex).is_empty(), "{fs:#?}");
    assert!(of_kind(&fs, FactKind::CtxRequirement).is_empty(), "{fs:#?}");
}

#[test]
fn resolved_query_shape_is_shown() {
    // Neither `list` nor the target `Product` appears in the signature — both are
    // inferred from the return shape + cardinality.
    let fs = facts_of(
        r#"
        Product { name: text, @index name }
        shape P from Product { name }
        query products() -> P[] order (name);
    "#,
    );
    let q = of_kind(&fs, FactKind::ResolvedQuery);
    assert_eq!(q.len(), 1, "{fs:#?}");
    assert_eq!(q[0].label, "list Product[]");
}

#[test]
fn get_query_resolves_to_singular() {
    let fs = facts_of(
        r#"
        Product { sku: text (unique), name: text }
        shape P from Product { name }
        query product(sku) -> P;
    "#,
    );
    let q = of_kind(&fs, FactKind::ResolvedQuery);
    assert_eq!(q.len(), 1, "{fs:#?}");
    assert_eq!(q[0].label, "get Product");
}

#[test]
fn ctx_requirement_is_shown_typed() {
    // `$ctx.org` compares against a `-> Org` relation column, so the request-context
    // field is inferred as a relation  — nothing in source declares it.
    let fs = facts_of(
        r#"
        Org { name: text }
        Product { org: Org, name: text, @index org }
        shape P from Product { name }
        query my_products() -> P[] where (org = $ctx.org) order (name);
    "#,
    );
    let ctx = of_kind(&fs, FactKind::CtxRequirement);
    assert_eq!(ctx.len(), 1, "{fs:#?}");
    assert_eq!(ctx[0].label, "requires [org: -> Org]");
    assert!(
        ctx[0].detail.contains("generated client"),
        "{}",
        ctx[0].detail
    );
}
