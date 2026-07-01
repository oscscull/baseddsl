//! Sema tests: parse a snippet, run `check`, and assert on the diagnostics and
//! the resolved schema. Snippets are whole (multi-decl) schemas so cross-model
//! resolution (relations, inverses, return types) is exercised end to end.

use based_ast::{FileId, Verb};
use based_diagnostics::{Diagnostic, Severity};
use based_parser::parse_file;
use based_sema::{check, CheckedSchema, MemberKind, SoftMode};

fn analyze(src: &str) -> (CheckedSchema, Vec<Diagnostic>) {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    check(&sf.decls)
}

fn codes(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code).collect()
}

fn errors(diags: &[Diagnostic]) -> Vec<&str> {
    diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code)
        .collect()
}

fn assert_clean(src: &str) {
    let (_, diags) = analyze(src);
    assert!(
        diags.is_empty(),
        "expected no diagnostics, got: {:?}",
        codes(&diags)
    );
}

// ---------- happy path -----------------------------------------------------

#[test]
fn minimal_schema_is_clean() {
    assert_clean(
        r#"
        @soft_delete(deleted_at)
        @sort(name asc)
        Org {
          deleted_at: timestamp?
          name: text
          slug: text (unique)
        }
        shape OrgCard from Org { name, slug }
        query org_by_id(id) -> OrgCard;
        query orgs() -> OrgCard[];
        "#,
    );
}

#[test]
fn resolved_schema_shape() {
    let (schema, diags) = analyze(
        r#"
        @soft_delete(deleted_at)
        @created(created_at)
        Order {
          deleted_at: timestamp?
          created_at: timestamp
          placed_by: User
          items: OrderItem[]
        }
        OrderItem { order: Order, qty: int }
        User {
          name: text
          placed_orders: Order[] (Order.placed_by)
        }
        "#,
    );
    assert!(
        errors(&diags).is_empty(),
        "unexpected errors: {:?}",
        codes(&diags)
    );

    let order = schema.model("Order").expect("Order resolved");
    assert_eq!(order.table, "order"); // snake_case, no pluralization (D3)
    assert_eq!(order.created.as_deref(), Some("created_at"));
    assert!(matches!(
        order.soft_delete.as_ref().unwrap().mode,
        SoftMode::Timestamp
    ));
    // implicit `id` is prepended (D2)
    assert!(matches!(
        order.member("id").unwrap().kind,
        MemberKind::Scalar { .. }
    ));
    // forward relation -> FK column `placed_by_id`
    match &order.member("placed_by").unwrap().kind {
        MemberKind::Forward { fk_col, target, .. } => {
            assert_eq!(fk_col, "placed_by_id");
            assert_eq!(target, "User");
        }
        k => panic!("expected forward relation, got {k:?}"),
    }
    // to-many `items` with no explicit ref infers the inverse via OrderItem.order
    match &order.member("items").unwrap().kind {
        MemberKind::Inverse { via, target } => {
            assert_eq!(via, "order");
            assert_eq!(target, "OrderItem");
        }
        k => panic!("expected inferred inverse, got {k:?}"),
    }

    assert_eq!(schema.model("OrderItem").unwrap().table, "order_item");
}

#[test]
fn query_verb_inference() {
    let (schema, diags) = analyze(
        r#"
        User { name: text }
        shape UserCard from User { name }
        query user_by_id(id) -> UserCard;
        query users() -> UserCard[];
        "#,
    );
    assert!(errors(&diags).is_empty(), "{:?}", codes(&diags));
    let get = schema
        .queries
        .iter()
        .find(|q| q.name == "user_by_id")
        .unwrap();
    assert_eq!(get.verb, Verb::Get);
    assert_eq!(get.target, "User");
    assert_eq!(get.ret_shape.as_deref(), Some("UserCard"));
    let list = schema.queries.iter().find(|q| q.name == "users").unwrap();
    assert_eq!(list.verb, Verb::List);
    assert!(list.many);
}

// ---------- resolution errors ---------------------------------------------

#[test]
fn unknown_model_in_relation() {
    let (_, d) = analyze("User { org: Org }");
    assert_eq!(errors(&d), ["E0110"]);
}

#[test]
fn unknown_field_in_where() {
    let (_, d) = analyze(
        r#"
        Product { name: text }
        shape P from Product { name }
        query find(org: Id) -> P[] { list Product where (missing = $org); }
        "#,
    );
    assert!(errors(&d).contains(&"E0111"), "{:?}", codes(&d));
}

#[test]
fn cannot_traverse_scalar() {
    let (_, d) = analyze(
        r#"
        Product { name: text }
        shape P from Product { deep = name.oops }
        "#,
    );
    assert_eq!(errors(&d), ["E0112"]);
}

#[test]
fn unknown_param_in_where() {
    let (_, d) = analyze(
        r#"
        Product { name: text }
        shape P from Product { name }
        query find() -> P[] { list Product where (name = $nope); }
        "#,
    );
    // `list` with a model @sort absent also warns; assert the error is present.
    assert!(errors(&d).contains(&"E0113"), "{:?}", codes(&d));
}

// ---------- decorators -----------------------------------------------------

#[test]
fn soft_delete_bad_type() {
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Doc { deleted_at: int }
        "#,
    );
    assert_eq!(errors(&d), ["E0120"]);
}

#[test]
fn soft_delete_requires_nullable_timestamp() {
    // non-nullable timestamp is not the covered subset
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Doc { deleted_at: timestamp }
        "#,
    );
    assert_eq!(errors(&d), ["E0120"]);
}

#[test]
fn unknown_decorator_warns() {
    let (_, d) = analyze("@wat\nDoc { name: text }");
    assert_eq!(codes(&d), ["W0101"]);
}

#[test]
fn index_unknown_column() {
    let (_, d) = analyze("Doc { name: text\n @index nope }");
    assert_eq!(errors(&d), ["E0122"]);
}

// ---------- inverses -------------------------------------------------------

#[test]
fn inverse_ambiguous() {
    let (_, d) = analyze(
        r#"
        User { invites: Membership[] }
        Membership { inviter: User, invitee: User }
        "#,
    );
    assert_eq!(errors(&d), ["E0124"]);
}

#[test]
fn inverse_ref_not_forward_edge() {
    let (_, d) = analyze(
        r#"
        User { posts: Post[] (Post.nope) }
        Post { author: User }
        "#,
    );
    assert_eq!(errors(&d), ["E0123"]);
}

// ---------- shapes ---------------------------------------------------------

#[test]
fn shape_bare_relation_rejected() {
    let (_, d) = analyze(
        r#"
        User { org: Org }
        Org { name: text }
        shape U from User { org }
        "#,
    );
    assert_eq!(errors(&d), ["E0130"]);
}

#[test]
fn shape_nest_scalar_rejected() {
    let (_, d) = analyze(
        r#"
        User { name: text }
        shape U from User { name { x } }
        "#,
    );
    assert_eq!(errors(&d), ["E0131"]);
}

#[test]
fn shape_nest_and_reach_ok() {
    assert_clean(
        r#"
        User { name: text, org: Org }
        Org { name: text, slug: text }
        shape U from User {
          name
          city = org.name
          org { name slug }
        }
        "#,
    );
}

// ---------- queries / mutations -------------------------------------------

#[test]
fn get_must_be_keyed_on_unique() {
    let (_, d) = analyze(
        r#"
        Product { name: text }
        shape P from Product { name }
        query by_name(name) -> P;
        "#,
    );
    assert_eq!(errors(&d), ["E0144"]);
}

#[test]
fn get_on_unique_column_ok() {
    assert_clean(
        r#"
        Product { sku: text (unique) }
        shape P from Product { sku }
        query by_sku(sku) -> P;
        "#,
    );
}

#[test]
fn unknown_return_type() {
    let (_, d) = analyze("query q(id) -> Nope;");
    assert_eq!(errors(&d), ["E0140"]);
}

#[test]
fn edge_binding_must_be_relation() {
    let (_, d) = analyze(
        r#"
        Product { name: text }
        shape P from Product { name }
        query q(x -> name) -> P[];
        "#,
    );
    assert_eq!(errors(&d), ["E0143"]);
}

#[test]
fn restore_requires_soft_delete() {
    let (_, d) = analyze(
        r#"
        Doc { name: text }
        shape D from Doc { name }
        mutation undo(id: Id) -> D { restore Doc where (id = $id); }
        "#,
    );
    assert_eq!(errors(&d), ["E0145"]);
}

#[test]
fn mutation_create_unknown_column() {
    let (_, d) = analyze(
        r#"
        Doc { name: text }
        shape D from Doc { name }
        mutation make(t: text) -> D { create Doc { nope = $t }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0111"]);
}

// ---------- filters --------------------------------------------------------

#[test]
fn filter_arity_mismatch() {
    let (_, d) = analyze(
        r#"
        Product { name: text, active: bool }
        shape P from Product { name }
        filter live() = active;
        query q() -> P[] { list Product where (live(1)) order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0115"), "{:?}", codes(&d));
}

#[test]
fn unknown_filter_call() {
    let (_, d) = analyze(
        r#"
        Product { name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (ghost()) order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0114"), "{:?}", codes(&d));
}

#[test]
fn unknown_function() {
    let (_, d) = analyze(
        r#"
        Product { name: text (default bogus()) }
        "#,
    );
    assert_eq!(errors(&d), ["E0116"]);
}

// ---------- lints ----------------------------------------------------------

#[test]
fn nondeterministic_list_warns() {
    let (_, d) = analyze(
        r#"
        Product { name: text }
        shape P from Product { name }
        query all() -> P[];
        "#,
    );
    assert_eq!(codes(&d), ["W0100"]);
}

#[test]
fn model_sort_silences_nondeterministic_lint() {
    assert_clean(
        r#"
        @sort(name asc)
        Product { name: text }
        shape P from Product { name }
        query all() -> P[];
        "#,
    );
}

#[test]
fn raw_soft_delete_gap_warns() {
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Product { deleted_at: timestamp?, name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (sql`name is not null`) order (name); }
        "#,
    );
    assert_eq!(codes(&d), ["W0102"]);
}

// ---------- duplicates -----------------------------------------------------

#[test]
fn duplicate_model_and_field() {
    let (_, d) = analyze("Doc { name: text, name: int }\nDoc { x: int }");
    assert!(errors(&d).contains(&"E0104"));
    assert!(errors(&d).contains(&"E0100"));
}
