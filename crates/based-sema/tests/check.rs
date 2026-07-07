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
        Doc { name: text? }
        shape D from Doc { name }
        mutation make(t: text) -> D { create Doc { nope = $t }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0111"]);
}

#[test]
fn mutation_create_missing_required_field() {
    // `title` is a non-optional, non-defaulted column, so a create that omits it
    // is `E0146`. `id`/soft-delete/`@created`/`@updated` are engine-set, exempt.
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        @created(created_at)
        Doc { deleted_at: timestamp?, created_at: timestamp, title: text, note: text? }
        shape D from Doc { title }
        mutation make(n: text) -> D { create Doc { note = $n }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0146"]);
}

#[test]
fn mutation_create_missing_required_relation_fk() {
    // A non-optional forward relation must have its FK set on create.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        Doc { org: Org, title: text }
        shape D from Doc { title }
        mutation make(t: text) -> D { create Doc { title = $t }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0146"]);
}

#[test]
fn mutation_create_defaulted_and_optional_not_required() {
    // Defaulted, optional, and engine-managed columns need no assignment.
    assert_clean(
        r#"
        @soft_delete(deleted_at)
        @updated(updated_at)
        Doc {
          deleted_at: timestamp?
          updated_at: timestamp
          title: text
          status: text (default "draft")
          note: text?
        }
        shape D from Doc { title }
        mutation make(t: text) -> D { create Doc { title = $t }; }
        "#,
    );
}

#[test]
fn mutation_create_wrong_literal_type_rejected() {
    // `count` is an int column; assigning a text literal is `E0153` — the write-side
    // twin of the `=` operand-typing on the read side.
    let (_, d) = analyze(
        r#"
        Doc { count: int, title: text }
        shape D from Doc { title }
        mutation make(t: text) -> D { create Doc { count = "lots", title = $t }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
}

#[test]
fn mutation_update_wrong_column_type_rejected() {
    // Assigning one column to another of an incompatible family (text ← int).
    let (_, d) = analyze(
        r#"
        Doc { count: int, title: text }
        shape D from Doc { title }
        mutation rename(id: Id) -> D { update Doc where (id = $id) { title = count }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
}

#[test]
fn mutation_assign_relation_key_is_clean() {
    // A forward FK accepts its key as a uuid string or an int (D1); a param or a
    // matching literal is fine. Correct scalar types pass too.
    assert_clean(
        r#"
        Org { name: text }
        Doc { org: Org, count: int, title: text }
        shape D from Doc { title }
        mutation make(o: Org, t: text) -> D {
          create Doc { org = $o, count = 3, title = $t };
        }
        "#,
    );
}

#[test]
fn tx_backref_type_mismatch_rejected() {
    // `^.count` reads an int off the preceding create; assigning it to a text column
    // is a family clash (`E0153`), typed through the back-reference.
    let (_, d) = analyze(
        r#"
        Batch { count: int }
        Doc { label: text, batch: Batch }
        shape D from Doc { label }
        mutation run(n: int) -> D {
          tx {
            create Batch { count = $n };
            create Doc { label = ^.count, batch = ^.id };
          }
        }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
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
fn filter_body_resolves_at_call_site() {
    // The filter body's columns (`active`, `stock`) belong to no model at its
    // declaration; they resolve against Product when the query calls it.
    assert_clean(
        r#"
        Product { name: text, active: bool, stock: int, @index(active, stock) }
        shape P from Product { name }
        filter sellable = active and stock > 0;
        query q() -> P[] { list Product where (sellable) order (name); }
        "#,
    );
}

#[test]
fn filter_body_bad_column_at_call_site() {
    // `stock` is not a column on Product — the filter body fails to resolve
    // against the call-site model even though the declaration itself parsed.
    let (_, d) = analyze(
        r#"
        Product { name: text, active: bool }
        shape P from Product { name }
        filter sellable = active and stock > 0;
        query q() -> P[] { list Product where (sellable) order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0111"), "{:?}", codes(&d));
}

#[test]
fn filter_body_traverses_relation_at_call_site() {
    // A relation-reaching filter path resolves through the call-site model's edges.
    assert_clean(
        r#"
        City { name: text }
        Address { city: City }
        User { name: text, address: Address }
        shape U from User { name }
        filter in_city(c) = address.city.name = $c;
        query users_in(c) -> U[] { list User where (in_city($c)) order (name); }
        "#,
    );
}

#[test]
fn filter_body_operand_type_checked_at_call_site() {
    // Operand typing rides the same call-site resolution: `~` on an int column
    // is caught inside the filter body.
    let (_, d) = analyze(
        r#"
        Product { name: text, stock: int }
        shape P from Product { name }
        filter cheap = stock ~ "x";
        query q() -> P[] { list Product where (cheap) order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0150"), "{:?}", codes(&d));
}

#[test]
fn recursive_filter_terminates() {
    // A self-referential filter must not loop forever; the cycle guard stops
    // re-expansion. (Whether to *reject* recursion is a separate policy.)
    let (_, d) = analyze(
        r#"
        Product { name: text, active: bool }
        shape P from Product { name }
        filter loopy = active and loopy;
        query q() -> P[] { list Product where (loopy) order (name); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
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

// ---------- operand typing (PLAN.md sema #1) -------------------------------

#[test]
fn like_on_non_text_rejected() {
    // `~` needs a text column; `qty` is an int.
    let (_, d) = analyze(
        r#"
        Product { name: text, qty: int }
        shape P from Product { name }
        query q(x) -> P[] { list Product where (qty ~ "%a%") order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0150"), "{:?}", codes(&d));
}

#[test]
fn like_on_text_ok() {
    assert_clean(
        r#"
        Product { name: text, @index name }
        shape P from Product { name }
        query q() -> P[] { list Product where (name ~ "%a%") order (name); }
        "#,
    );
}

#[test]
fn ordering_on_relation_rejected() {
    // `<` on a relation edge is nonsense.
    let (_, d) = analyze(
        r#"
        Product { name: text, maker: Maker }
        Maker { name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (maker < "x") order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0150"), "{:?}", codes(&d));
}

#[test]
fn literal_type_mismatch_rejected() {
    // comparing an int column to a string literal.
    let (_, d) = analyze(
        r#"
        Product { name: text, qty: int }
        shape P from Product { name }
        query q() -> P[] { list Product where (qty = "lots") order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0151"), "{:?}", codes(&d));
}

#[test]
fn bool_column_vs_number_rejected() {
    let (_, d) = analyze(
        r#"
        Product { name: text, active: bool }
        shape P from Product { name }
        query q() -> P[] { list Product where (active > 3) order (name); }
        "#,
    );
    // ordering on a bool column is not orderable.
    assert!(errors(&d).contains(&"E0150"), "{:?}", codes(&d));
}

#[test]
fn relation_compared_to_uuid_literal_ok() {
    // a relation edge is comparable to its key (a uuid string).
    assert_clean(
        r#"
        Product { name: text, maker: Maker, @index maker }
        Maker { name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (maker = "0f-uuid") order (name); }
        "#,
    );
}

#[test]
fn column_vs_column_family_mismatch_rejected() {
    let (_, d) = analyze(
        r#"
        Product { name: text, qty: int }
        shape P from Product { name }
        query q() -> P[] { list Product where (qty = name) order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0151"), "{:?}", codes(&d));
}

#[test]
fn param_type_disagrees_with_column_rejected() {
    // `id` param annotated `int` but maps to a text column.
    let (_, d) = analyze(
        r#"
        Product { sku: text }
        shape P from Product { sku }
        query q(sku: int) -> P[];
        "#,
    );
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn relation_param_typed_as_model_ok() {
    assert_clean(
        r#"
        @sort(name asc)
        Product { name: text, maker: Maker, @index maker }
        Maker { name: text }
        shape P from Product { name }
        query by_maker(maker: Maker) -> P[];
        "#,
    );
}

#[test]
fn relation_param_typed_wrong_model_rejected() {
    let (_, d) = analyze(
        r#"
        Product { name: text, maker: Maker }
        Maker { name: text }
        Other { name: text }
        shape P from Product { name }
        query by_maker(maker: Other) -> P[];
        "#,
    );
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn relation_param_typed_as_id_ok() {
    // D1: a relation param may be typed as its key (`Id`) instead of the model.
    assert_clean(
        r#"
        @sort(name asc)
        Product { name: text, maker: Maker, @index maker }
        Maker { name: text }
        shape P from Product { name }
        query by_maker(maker: Id) -> P[];
        "#,
    );
}

// ---------- index inference + lints (indexing.md, D15) ----------------------

#[test]
fn traversed_inverse_edge_infers_join_key() {
    // The shape reaches `items.qty` (an inverse hop), so OrderItem needs an index
    // on the FK the join runs through — inferred, not declared.
    let (schema, diags) = analyze(
        r#"
        @sort(placed_at desc)
        Order { placed_at: timestamp, items: OrderItem[], @index placed_at }
        OrderItem { order: Order, qty: int }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#,
    );
    assert!(diags.is_empty(), "{:?}", codes(&diags));
    let inferred = &schema.model("OrderItem").unwrap().inferred_indexes;
    assert_eq!(inferred.len(), 1, "{inferred:?}");
    assert_eq!(inferred[0].columns, vec!["order".to_string()]);
}

#[test]
fn inferred_join_key_deduped_by_declared_index() {
    // The user already declared the join-key index; nothing is inferred, and the
    // declared index counts as used (no W0104).
    let (schema, diags) = analyze(
        r#"
        @sort(placed_at desc)
        Order { placed_at: timestamp, items: OrderItem[], @index placed_at }
        OrderItem { order: Order, qty: int, @index order }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#,
    );
    assert!(diags.is_empty(), "{:?}", codes(&diags));
    assert!(schema
        .model("OrderItem")
        .unwrap()
        .inferred_indexes
        .is_empty());
}

#[test]
fn unindexed_query_warns() {
    // `by_status` filters `status`, but no index leads with it — the query scans.
    let (_, d) = analyze(
        r#"
        Product { name: text, status: text, @index name }
        shape P from Product { name }
        query by_status(status) -> P[] order (name);
        "#,
    );
    assert_eq!(codes(&d), vec!["W0103"]);
}

#[test]
fn unindexed_satisfied_by_declared_index() {
    assert_clean(
        r#"
        Product { name: text, status: text, @index status }
        shape P from Product { name }
        query by_status(status) -> P[] order (name);
        "#,
    );
}

#[test]
fn unindexed_satisfied_by_unsafe_annotation() {
    // The loud opt-out: greppable, silences W0103, never silently dropped.
    assert_clean(
        r#"
        Product { name: text, status: text, @index name }
        shape P from Product { name }
        query by_status(status) -> P[] order (name) unindexed(unsafe, "ops table, stays tiny");
        "#,
    );
}

#[test]
fn unindexed_satisfied_by_max_rows_in_block() {
    assert_clean(
        r#"
        Product { name: text, status: text, @index name }
        shape P from Product { name }
        query by_status(s) -> P[] {
          list Product where (status = $s) order (name) unindexed(max_rows: 500);
        }
        "#,
    );
}

#[test]
fn mutation_where_unindexed_warns() {
    // A bulk `update` filtering a non-unique, unindexed column scans just like a
    // query would — W0103 (mutations carry no `unindexed(…)` clause to suppress it).
    let (_, d) = analyze(
        r#"
        Product { name: text, status: text }
        shape P from Product { name }
        mutation archive(s: text) -> P { update Product where (status = $s) { name = "x" }; }
        "#,
    );
    assert_eq!(codes(&d), vec!["W0103"]);
}

#[test]
fn mutation_where_keyed_on_unique_is_clean() {
    // The common case: a write keyed on `id` (unique) is served, no W0103.
    assert_clean(
        r#"
        Product { name: text, status: text }
        shape P from Product { name }
        mutation rename(id: Id, n: text) -> P { update Product where (id = $id) { name = $n }; }
        "#,
    );
}

#[test]
fn mutation_where_marks_index_used() {
    // An index a mutation's `where` relies on is not useless: feeding writes into
    // the usage pool keeps W0104 from firing on a mutation-only index.
    assert_clean(
        r#"
        Product { name: text, status: text, @index status }
        shape P from Product { name }
        mutation archive(s: text) -> P { update Product where (status = $s) { name = "x" }; }
        "#,
    );
}

#[test]
fn stale_unindexed_annotation_warns() {
    // `sku` is unique, so the get is indexed — the annotation is stale.
    let (_, d) = analyze(
        r#"
        Product { sku: text (unique), name: text }
        shape P from Product { name }
        query by_sku(sku) -> P unindexed(max_rows: 10);
        "#,
    );
    assert_eq!(codes(&d), vec!["W0105"]);
}

#[test]
fn useless_index_warns() {
    // Nothing filters, sorts, or joins on `price`; the index is pure write tax.
    let (_, d) = analyze(
        r#"
        @sort(name asc)
        Product { name: text, price: int, @index price, @index name }
        shape P from Product { name }
        query all() -> P[];
        "#,
    );
    assert_eq!(codes(&d), vec!["W0104"]);
}

#[test]
fn paginated_sort_wants_an_index() {
    // No filter at all: a paginated list still pays for its sort — W0103 unless
    // the sort key is indexed.
    let (_, d) = analyze(
        r#"
        Product { name: text, created_at: timestamp }
        shape P from Product { name }
        query recent() -> P[] order (created_at desc) page (20);
        "#,
    );
    assert_eq!(codes(&d), vec!["W0103"]);
    assert_clean(
        r#"
        Product { name: text, created_at: timestamp, @index created_at }
        shape P from Product { name }
        query recent() -> P[] order (created_at desc) page (20);
        "#,
    );
}

#[test]
fn unique_index_is_never_useless() {
    // A unique index is a constraint, not a perf structure — exempt from W0104
    // even with no queries at all.
    assert_clean("M { a: text, b: text, @index(a, b) unique }");
}

#[test]
fn index_duplicating_unique_constraint_warns() {
    let (_, d) = analyze("Org { slug: text (unique), @index slug }");
    assert_eq!(codes(&d), vec!["W0104"]);
}

#[test]
fn or_predicate_is_opaque_to_unindexed() {
    // First-column reasoning can't judge an `or`; W0103 stays silent rather than
    // guess (precision over recall).
    assert_clean(
        r#"
        Product { name: text, a: text, b: text, @index name }
        shape P from Product { name }
        query q() -> P[] { list Product where (a = "x" or b = "y") order (name); }
        "#,
    );
}

#[test]
fn scope_filter_counts_toward_pattern() {
    // `@scope` is injected into every query on the model (auth.md), so its
    // columns are part of every query's index pattern.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert_eq!(codes(&d), vec!["W0103"]);
    assert_clean(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
}

// ---------- named scope: decl / @scope / scoped (D46/D47) ------------------

#[test]
fn scope_term_must_bind_ctx_field() {
    // A `scope` decl term binds `col: Type = $ctx.<field>`; a non-`$ctx` binding is
    // `E0180` (the predicate-form rule, now at the decl site).
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $other.org)
        @scope Tenant
        Doc { org: Org, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0180"), "{:?}", codes(&d));

    // A multi-segment `$ctx` path is not the flat scope-field form → `E0180`.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org.id)
        @scope Tenant
        Doc { org: Org, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0180"), "{:?}", codes(&d));
}

#[test]
fn multi_term_ctx_equality_scope_is_clean() {
    // A conjunction of `col = $ctx.field` equalities is the allowed shape (one decl
    // with two terms, D46).
    assert_clean(
        r#"
        Org { name: text }
        Region { name: text }
        scope Tenant (org: Org = $ctx.org, region: Region = $ctx.region)
        @scope Tenant
        Doc { org: Org, region: Region, title: text, @index(org, region) }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant { list Doc order (title); }
        "#,
    );
}

#[test]
fn scoped_callable_must_acknowledge_scope_e0182() {
    // A callable touching a scoped model with neither `scoped …` nor `unscoped(…)` is
    // `E0182` — the contract is written, not implied.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0182"), "{:?}", codes(&d));
}

#[test]
fn scope_ref_unknown_name_is_e0183() {
    // `@scope Name` / `scoped Name` naming no `scope` decl is `E0183`.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        @scope Nope
        Doc { org: Org, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Nope order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0183"), "{:?}", codes(&d));
}

#[test]
fn scope_model_missing_or_wrong_column_is_e0184() {
    // A `@scope` model must carry the scope's column at a conforming type. Here `Doc`
    // has no `org` field → `E0184`.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0184"), "{:?}", codes(&d));

    // A column of the wrong type also fails E0184 (`org` is text, not the relation `Org`).
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: text, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0184"), "{:?}", codes(&d));
}

#[test]
fn scoped_naming_untouched_scope_is_e0185() {
    // `scoped Other` names a scope no model this callable touches declares → `E0185`.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Other (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] scoped Other order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0185"), "{:?}", codes(&d));
}

#[test]
fn scope_field_type_sourced_from_the_decl() {
    // The scope field's `$ctx` type comes from the decl (`org: Org`), so a scoped query
    // requires `$ctx.org` typed as an `Org` relation — no per-callable inference needed.
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let scope = schema.scope("Tenant").expect("Tenant scope resolved");
    assert_eq!(scope.terms.len(), 1);
    assert_eq!(scope.terms[0].column, "org");
    assert_eq!(scope.terms[0].ctx_field, "org");
    let docs = schema.queries.iter().find(|q| q.name == "docs").unwrap();
    assert_eq!(docs.ctx_requires.len(), 1);
    assert_eq!(docs.ctx_requires[0].field, "org");
}

#[test]
fn create_assigning_scope_column_is_e0181() {
    // The scope column is engine-managed on create (auto-set from $ctx); a caller
    // that assigns it is trying to plant a row into an arbitrary scope.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        mutation make(org: Id, title: text) -> D scoped Tenant {
          create Doc { org = $org, title = $title };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0181"), "{:?}", codes(&d));
}

#[test]
fn create_on_scoped_model_omitting_scope_column_is_clean() {
    // The scope column is required-exempt (E0146) because the engine auto-sets it.
    // The create's ctx bag gains `org` from that auto-set.
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        mutation make(title: text) -> D scoped Tenant { create Doc { title = $title }; }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let make = schema.mutations.iter().find(|m| m.name == "make").unwrap();
    assert_eq!(make.ctx_requires.len(), 1);
    assert_eq!(make.ctx_requires[0].field, "org");
}

#[test]
fn unscoped_query_drops_the_scope_ctx_requirement() {
    // An `unscoped` query opts out of `@scope` injection, so it no longer requires
    // the scope's `$ctx` field.
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query all(org) -> D[] unscoped("admin: cross-org read") order (title);
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let all = schema.queries.iter().find(|q| q.name == "all").unwrap();
    assert!(all.ctx_requires.is_empty(), "{:?}", all.ctx_requires);
}

#[test]
fn unscoped_create_may_assign_the_scope_column() {
    // With the mutation `unscoped`, scope isn't injected/auto-set, so the caller owns
    // the column and assigning it is fine (no E0181).
    assert_clean(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text }
        shape D from Doc { title }
        mutation import_doc(org: Id, title: text) -> D
          unscoped("data import: rows land in the supplied org") {
          create Doc { org = $org, title = $title };
        }
        "#,
    );
}

#[test]
fn unscoped_on_a_model_without_scope_is_stale_w0106() {
    let (_, d) = analyze(
        r#"
        Doc { title: text, @index title }
        shape D from Doc { title }
        query all() -> D[] unscoped("nothing to opt out of") order (title);
        "#,
    );
    assert_eq!(codes(&d), vec!["W0106"]);
}

// ---------- $ctx inference + coherence (D4/D5) -----------------------------

#[test]
fn ctx_inferred_from_use_is_clean() {
    // No declaration anywhere: `$ctx.org`'s type is inferred from the `org` column
    // it compares against (auth.md Handle 1).
    assert_clean(
        r#"
        Org { name: text }
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx.org) order (title); }
        "#,
    );
}

#[test]
fn ctx_requirement_is_recorded_per_callable() {
    // The inferred requirement is attached to the callable that reads it — the
    // client sends exactly this as request context.
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx.org) order (title); }
        query all() -> D[] order (title);
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let docs = schema.queries.iter().find(|q| q.name == "docs").unwrap();
    assert_eq!(docs.ctx_requires.len(), 1);
    assert_eq!(docs.ctx_requires[0].field, "org");
    // a query that reads no context requires none
    let all = schema.queries.iter().find(|q| q.name == "all").unwrap();
    assert!(all.ctx_requires.is_empty());
}

#[test]
fn ctx_scope_propagates_to_every_query() {
    // `@scope` reads `$ctx.org`, so every query on the model requires it even with
    // no `where` of its own (auth.md Handle 2).
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let docs = schema.queries.iter().find(|q| q.name == "docs").unwrap();
    assert_eq!(docs.ctx_requires.len(), 1);
    assert_eq!(docs.ctx_requires[0].field, "org");
}

#[test]
fn ctx_joined_scope_is_required_via_shape_reach() {
    // D34: a query on an *unscoped* model that reaches a *scoped* model through a
    // shape relation joins it, and codegen injects the joined model's `@scope` into
    // the join `ON` — so the callable must require that model's `$ctx.org`, else the
    // injected `:ctx_org` bind is unbound at runtime.
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { org: Org, name: text }
        Ticket { raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query ticket_by_id(id) -> TicketCard scoped Tenant;
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "ticket_by_id")
        .unwrap();
    assert_eq!(q.ctx_requires.len(), 1, "{:?}", q.ctx_requires);
    assert_eq!(q.ctx_requires[0].field, "org");
}

#[test]
fn ctx_joined_scope_is_required_via_where_reach() {
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { org: Org, name: text, @index name }
        Ticket { raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject }
        query tickets(name) -> TicketCard[] scoped Tenant {
          list Ticket where (raised_by.name = $name) order (subject);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema.queries.iter().find(|q| q.name == "tickets").unwrap();
    assert!(
        q.ctx_requires.iter().any(|r| r.field == "org"),
        "{:?}",
        q.ctx_requires
    );
}

#[test]
fn ctx_unscoped_query_drops_joined_scope_requirement() {
    // `unscoped` (D32) drops all scope handling, joins included, so the joined
    // scoped model contributes no `$ctx` requirement (mirrors codegen D34).
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { org: Org, name: text }
        Ticket { raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query any_ticket(id) -> TicketCard unscoped("admin: cross-org lookup");
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "any_ticket")
        .unwrap();
    assert!(q.ctx_requires.is_empty(), "{:?}", q.ctx_requires);
}

#[test]
fn ctx_bad_path_errors() {
    // `$ctx` fields are flat: exactly one segment.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx.org.deep) order (title); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0160"]);
}

#[test]
fn ctx_bare_no_field_errors() {
    let (_, d) = analyze(
        r#"
        Org { name: text }
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx) order (title); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0160"]);
}

#[test]
fn ctx_coherent_across_callables_is_clean() {
    // `$ctx.org` is an `Org` key in both queries — one coherent request-context bag.
    assert_clean(
        r#"
        Org { name: text }
        Doc { org: Org, title: text, @index org }
        shape D from Doc { title }
        query a() -> D[] { list Doc where (org = $ctx.org) order (title); }
        query b() -> D[] { list Doc where (org = $ctx.org) order (title); }
        "#,
    );
}

#[test]
fn ctx_conflict_across_callables_errors() {
    // `$ctx.org` is an `Org` key in `a` but a text value in `b` — the caller can't
    // build one bag that satisfies both.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        Doc { org: Org, title: text, @index org, @index title }
        shape D from Doc { title }
        query a() -> D[] { list Doc where (org = $ctx.org) order (title); }
        query b() -> D[] { list Doc where (title = $ctx.org) order (title); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0161"]);
}

#[test]
fn ctx_conflict_within_one_callable_errors() {
    // Same field used at two types in one query is itself incoherent.
    let (_, d) = analyze(
        r#"
        Org { name: text }
        Doc { org: Org, title: text, @index org, @index title }
        shape D from Doc { title }
        query a() -> D[] {
          list Doc where (org = $ctx.x and title = $ctx.x) order (title);
        }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0161"]);
}

#[test]
fn ctx_from_create_assign_is_recorded() {
    // A `create` can set a column from context; the field types from that column.
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        Doc { org: Org, title: text }
        shape D from Doc { title }
        mutation add(t: text) -> D { create Doc { org = $ctx.org, title = $t }; }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let add = schema.mutations.iter().find(|m| m.name == "add").unwrap();
    assert_eq!(add.ctx_requires.len(), 1);
    assert_eq!(add.ctx_requires[0].field, "org");
}

#[test]
fn ctx_mutation_reselect_joined_scope_is_required() {
    // D34: a mutation's declared-shape re-select (D12) projects the return shape, so a
    // relation reach in that shape joins a scoped model and injects its `@scope` — the
    // mutation must require the joined model's `$ctx.org` too. Here `Ticket` is
    // unscoped but its re-select reaches the org-scoped `Contact`.
    let (schema, d) = analyze(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { org: Org, name: text }
        Ticket { raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        mutation open_ticket(by: Contact, subject: text) -> TicketCard scoped Tenant {
          create Ticket { raised_by = $by, subject = $subject };
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let m = schema
        .mutations
        .iter()
        .find(|m| m.name == "open_ticket")
        .unwrap();
    assert!(
        m.ctx_requires.iter().any(|r| r.field == "org"),
        "{:?}",
        m.ctx_requires
    );
}

// ---------- tx back-references (`^`, mutations.md) --------------------------

#[test]
fn tx_backref_to_prior_create_is_clean() {
    assert_clean(
        r#"
        User { email: text }
        Address { user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email };
            create Address { user = ^.id, city = $city };
          }
        }
        "#,
    );
}

#[test]
fn tx_backref_to_unknown_field_rejected() {
    let (_, d) = analyze(
        r#"
        User { email: text }
        Address { user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email };
            create Address { user = ^.nope, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0111"), "{:?}", codes(&d));
}

#[test]
fn backref_without_prior_create_rejected() {
    // First statement in the tx: nothing precedes it to back-reference.
    let (_, d) = analyze(
        r#"
        Address { city: text, ref_id: text }
        shape A from Address { city }
        mutation m(city: text) -> A {
          tx {
            create Address { ref_id = ^.id, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0170"), "{:?}", codes(&d));
}

#[test]
fn backref_outside_tx_rejected() {
    // `^` in a plain (non-tx) create has no preceding step in scope.
    let (_, d) = analyze(
        r#"
        Address { city: text, ref_id: text }
        shape A from Address { city }
        mutation m(city: text) -> A {
          create Address { ref_id = ^.id, city = $city };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0170"), "{:?}", codes(&d));
}

#[test]
fn backref_in_query_predicate_rejected() {
    // `^` is only valid in a tx write; a query `where` is a misuse.
    let (_, d) = analyze(
        r#"
        Doc { title: text }
        shape D from Doc { title }
        query find() -> D[] { list Doc where (title = ^.id); }
        "#,
    );
    assert!(errors(&d).contains(&"E0170"), "{:?}", codes(&d));
}

// ---------- custom `on:` joins (relations.md, resume #5) --------------------

#[test]
fn custom_join_resolves_clean() {
    // A legacy-key join: both sides are table-qualified columns that resolve
    // against the FK-holding model (`order`) and its target (`user`).
    assert_clean(
        r#"
        Order {
          user_ref: int
          placed_by: User (on: order.user_ref = user.legacy_id)
        }
        User { name: text, legacy_id: int }
        "#,
    );
}

#[test]
fn custom_join_unknown_column_rejected() {
    // `user.nope` is not a column on the target model.
    let (_, d) = analyze(
        r#"
        Order {
          user_ref: int
          placed_by: User (on: order.user_ref = user.nope)
        }
        User { name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0111"), "{:?}", codes(&d));
}

#[test]
fn custom_join_unknown_table_rejected() {
    // `customers` names no table in the two-table join scope.
    let (_, d) = analyze(
        r#"
        Order {
          user_ref: int
          placed_by: User (on: order.user_ref = customers.legacy_id)
        }
        User { name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0125"), "{:?}", codes(&d));
}

#[test]
fn custom_join_unqualified_column_rejected() {
    // A join column must be `<table>.<column>`; a bare `user_ref` is malformed.
    let (_, d) = analyze(
        r#"
        Order {
          user_ref: int
          placed_by: User (on: user_ref = user.legacy_id)
        }
        User { name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0126"), "{:?}", codes(&d));
}

#[test]
fn custom_join_on_scalar_rejected() {
    // `on:` only makes sense on a to-one relation, not a scalar field.
    let (_, d) = analyze(
        r#"
        Order {
          user_ref: int (on: order.user_ref = user.legacy_id)
        }
        User { name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0126"), "{:?}", codes(&d));
}

#[test]
fn custom_join_param_rejected() {
    // A join is static structure — a request `$` param has no meaning here.
    let (_, d) = analyze(
        r#"
        Order {
          user_ref: int
          placed_by: User (on: order.user_ref = $x)
        }
        User { name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0126"), "{:?}", codes(&d));
}

// ---------- multi-scope DNF: alternatives, E0185, E0186 (D47) --------------

#[test]
fn or_model_query_injects_only_its_named_alternative() {
    // A model with two stacked `@scope` decorators is an OR of alternatives; each
    // query names one and injects only that axis (auth.md worked example).
    let (schema, diags) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { title: text }
        User { name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          page:    Page
          author:  User
          body:    text
          created: timestamp
        }
        shape PostCard from Post { body }
        query posts_on_page() -> PostCard[] scoped Page   { list Post order (created desc); }
        query my_posts()      -> PostCard[] scoped Author { list Post order (created desc); }
        "#,
    );
    assert!(errors(&diags).is_empty(), "{:?}", codes(&diags));
    let by_page = schema
        .queries
        .iter()
        .find(|q| q.name == "posts_on_page")
        .unwrap();
    let by_author = schema
        .queries
        .iter()
        .find(|q| q.name == "my_posts")
        .unwrap();
    // Each query resolved a *different* alternative for the same model.
    assert_eq!(by_page.scope_inject.len(), 1);
    assert_eq!(
        by_page.scope_inject[0].terms,
        vec![("page".into(), "page".into())]
    );
    assert_eq!(by_author.scope_inject.len(), 1);
    assert_eq!(
        by_author.scope_inject[0].terms,
        vec![("author".into(), "user".into())]
    );
}

#[test]
fn and_model_naming_one_axis_is_e0185() {
    // `@scope Page, Author` is a single two-axis alternative; a callable naming just
    // `Page` doesn't ⊇ it → E0185.
    let (_, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { title: text }
        User { name: text }
        @scope Page, Author
        @sort(created desc)
        Comment {
          page:    Page
          author:  User
          body:    text
          created: timestamp
        }
        shape CommentCard from Comment { body }
        query my_comments() -> CommentCard[] scoped Page {
          list Comment order (created desc);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0185"), "{:?}", codes(&d));
}

#[test]
fn and_model_naming_both_axes_is_clean() {
    let (schema, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { title: text }
        User { name: text }
        @scope Page, Author
        @sort(created desc)
        Comment {
          page:    Page
          author:  User
          body:    text
          created: timestamp
        }
        shape CommentCard from Comment { body }
        query my_comments() -> CommentCard[] scoped Page, Author {
          list Comment order (created desc);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "my_comments")
        .unwrap();
    // Both axes injected (the single alternative), in decl order.
    assert_eq!(
        q.scope_inject[0].terms,
        vec![
            ("page".into(), "page".into()),
            ("author".into(), "user".into())
        ]
    );
}

#[test]
fn create_not_satisfying_any_alternative_is_e0186() {
    // A `create` on an AND model whose mutation names only one axis can satisfy no
    // alternative — the other scope column would be left unset → E0186.
    let (_, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { title: text }
        User { name: text }
        @scope Page, Author
        Comment {
          page:   Page
          author: User
          body:   text
        }
        shape CommentCard from Comment { body }
        mutation add_comment(body: text) -> CommentCard scoped Page {
          create Comment { body = $body };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0186"), "{:?}", codes(&d));
}

#[test]
fn create_satisfying_an_alternative_has_no_e0186() {
    // Naming the full alternative (`scoped Page, Author`) lets the engine auto-set both
    // scope columns from $ctx — no E0186.
    let (_, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { title: text }
        User { name: text }
        @scope Page, Author
        Comment {
          page:   Page
          author: User
          body:   text
        }
        shape CommentCard from Comment { body }
        mutation add_comment(body: text) -> CommentCard scoped Page, Author {
          create Comment { body = $body };
        }
        "#,
    );
    assert!(!codes(&d).contains(&"E0186"), "{:?}", codes(&d));
}
