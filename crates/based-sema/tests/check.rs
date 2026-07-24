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
          id: Id
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
          id: Id
          deleted_at: timestamp?
          created_at: timestamp
          placed_by: User
          items: OrderItem[]
        }
        OrderItem { id: Id, order: Order, qty: int }
        User {
          id: Id
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
    assert_eq!(order.table, "order"); // snake_case, no pluralization
    assert_eq!(order.created.as_deref(), Some("created_at"));
    assert!(matches!(
        order.soft_delete.as_ref().unwrap().mode,
        SoftMode::Timestamp
    ));
    // implicit `id` is prepended
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
        User { id: Id, name: text }
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
    let (_, d) = analyze("User { id: Id, org: Org }");
    assert_eq!(errors(&d), ["E0110"]);
}

#[test]
fn unknown_field_in_where() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text }
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
        Product { id: Id, name: text }
        shape P from Product { deep = name.oops }
        "#,
    );
    assert_eq!(errors(&d), ["E0112"]);
}

#[test]
fn unknown_param_in_where() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text }
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
        Doc { id: Id, deleted_at: int }
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
        Doc { id: Id, deleted_at: timestamp }
        "#,
    );
    assert_eq!(errors(&d), ["E0120"]);
}

#[test]
fn unknown_decorator_warns() {
    let (_, d) = analyze("@wat\nDoc { id: Id, name: text }");
    assert_eq!(codes(&d), ["W0101"]);
}

#[test]
fn index_unknown_column() {
    let (_, d) = analyze("Doc { id: Id, name: text\n @index nope }");
    assert_eq!(errors(&d), ["E0122"]);
}

// ---------- inverses -------------------------------------------------------

#[test]
fn inverse_ambiguous() {
    let (_, d) = analyze(
        r#"
        User { id: Id, invites: Membership[] }
        Membership { id: Id, inviter: User, invitee: User }
        "#,
    );
    assert_eq!(errors(&d), ["E0124"]);
}

#[test]
fn inverse_ref_not_forward_edge() {
    let (_, d) = analyze(
        r#"
        User { id: Id, posts: Post[] (Post.nope) }
        Post { id: Id, author: User }
        "#,
    );
    assert_eq!(errors(&d), ["E0123"]);
}

// ---------- shapes ---------------------------------------------------------

#[test]
fn shape_bare_relation_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, org: Org }
        Org { id: Id, name: text }
        shape U from User { org }
        "#,
    );
    assert_eq!(errors(&d), ["E0130"]);
}

#[test]
fn shape_nest_scalar_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape U from User { name { x } }
        "#,
    );
    assert_eq!(errors(&d), ["E0131"]);
}

#[test]
fn shape_nest_and_reach_ok() {
    assert_clean(
        r#"
        User { id: Id, name: text, org: Org }
        Org { id: Id, name: text, slug: text }
        shape U from User {
          name
          city = org.name
          org { name slug }
        }
        "#,
    );
}

#[test]
fn shape_nest_ref_ok_to_one_and_to_many() {
    // A named-shape nest works on a forward (to-one) and an inverse (to-many)
    // relation, and a referenced shape may itself nest (inline or by name).
    assert_clean(
        r#"
        User { id: Id, name: text, org: Org, placed_orders: Order[] (Order.placed_by) }
        Org { id: Id, name: text, slug: text }
        Order { id: Id, placed_by: User, total: int }
        shape OrgRef from Org { name, slug }
        shape UserRef from User { name, org -> OrgRef }
        shape OrderDetail from Order {
          total
          placed_by -> UserRef
        }
        shape UserOrders from User {
          name
          placed_orders -> OrderRow
        }
        shape OrderRow from Order { total }
        query detail(id) -> OrderDetail;
        "#,
    );
}

#[test]
fn shape_nest_ref_unknown_shape() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        Order { id: Id, placed_by: User }
        shape D from Order { placed_by -> Missing }
        "#,
    );
    assert_eq!(errors(&d), ["E0132"]);
}

#[test]
fn shape_nest_ref_model_mismatch() {
    // `OrgRef` projects `Org`, but `placed_by` relates to `User` — never silent.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        Org { id: Id, name: text }
        Order { id: Id, placed_by: User }
        shape OrgRef from Org { name }
        shape D from Order { placed_by -> OrgRef }
        "#,
    );
    assert_eq!(errors(&d), ["E0133"]);
}

#[test]
fn shape_nest_ref_on_scalar_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape N from User { name }
        shape D from User { name -> N }
        "#,
    );
    assert_eq!(errors(&d), ["E0131"]);
}

#[test]
fn shape_nest_ref_cycle_rejected() {
    // `UserRef` -> `OrderRow` -> `UserRef` would expand forever; each decl reports
    // the reference that closes the cycle from its own root.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text, placed_orders: Order[] (Order.placed_by) }
        Order { id: Id, placed_by: User }
        shape UserRef from User { placed_orders -> OrderRow }
        shape OrderRow from Order { placed_by -> UserRef }
        "#,
    );
    assert!(
        errors(&d).contains(&"E0134"),
        "expected a cycle error, got: {:?}",
        codes(&d)
    );
}

#[test]
fn shape_nest_ref_self_cycle_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text, invited_by: User? }
        shape UserTree from User { name, invited_by -> UserTree }
        "#,
    );
    assert_eq!(errors(&d), ["E0134"]);
}

#[test]
fn shape_flatten_far_side_ok() {
    // `courses = enrollments.course { title }` — a to-many hop into the junction, then a
    // forward hop to the far side; the body projects the far model. Checks clean.
    assert_clean(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, course: Course, @index (student, course) }
        Course { id: Id, title: text }
        shape StudentCourses from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> StudentCourses;
        "#,
    );
}

#[test]
fn shape_flatten_first_segment_must_be_to_many() {
    // A forward (to-one) first segment has no junction to flatten through → E0300.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, school: School, @index school }
        School { id: Id, name: text }
        shape S from Student { name, x = school.name { name } }
        "#,
    );
    assert!(errors(&d).contains(&"E0300"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_later_segment_must_be_forward() {
    // After the junction hop, a non-forward segment (a scalar) is E0301.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, note: text, @index student }
        shape S from Student { name, x = enrollments.note { title } }
        "#,
    );
    assert!(errors(&d).contains(&"E0301"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_single_segment_has_no_far_side() {
    // A one-segment path never reaches a far side (nothing to flatten to) → E0301.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, note: text, @index student }
        shape S from Student { name, x = enrollments { note } }
        "#,
    );
    assert!(errors(&d).contains(&"E0301"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_keyless_far_side_rejected() {
    // A `@no_id` far model has no primary key to dedup the distinct set on → E0302.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, course: Course, @index (student, course) }
        @no_id("legacy view without a key")
        Course { title: text }
        shape S from Student { name, courses = enrollments.course { title } }
        "#,
    );
    assert!(errors(&d).contains(&"E0302"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_scoped_far_side_is_touched_e0185() {
    // Flattening into a scoped far side (or junction) counts as touching it: a callable
    // that doesn't satisfy that scope alternative fails at compile time.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        Student { id: Id, org: Org, name: text, enrollments: Enrollment[] (Enrollment.student), @index org }
        @scope Tenant
        Enrollment { id: Id, org: Org, student: Student, course: Course, @index (student, course), @index org }
        @scope Region
        Course { id: Id, region: Region, title: text, @index region }
        shape S from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> S scoped Tenant;
        "#,
    );
    assert!(errors(&d).contains(&"E0185"), "{:?}", codes(&d));

    // Naming both axes satisfies every touched model → clean.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        Student { id: Id, org: Org, name: text, enrollments: Enrollment[] (Enrollment.student), @index org }
        @scope Tenant
        Enrollment { id: Id, org: Org, student: Student, course: Course, @index (student, course), @index org }
        @scope Region
        Course { id: Id, region: Region, title: text, @index region }
        shape S from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> S scoped Tenant, Region;
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

// ---------- queries / mutations -------------------------------------------

#[test]
fn get_must_be_keyed_on_unique() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, @index name }
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
        Product { id: Id, sku: text (unique) }
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
        Product { id: Id, name: text, @index name }
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
        Doc { id: Id, name: text }
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
        Doc { id: Id, name: text? }
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
        Doc { id: Id, deleted_at: timestamp?, created_at: timestamp, title: text, note: text? }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text }
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
          id: Id
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
        Doc { id: Id, count: int, title: text }
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
        Doc { id: Id, count: int, title: text }
        shape D from Doc { title }
        mutation rename(id: Id) -> D { update Doc where (id = $id) { title = count }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
}

#[test]
fn mutation_assign_relation_key_is_clean() {
    // A forward FK accepts its key as a uuid string or an int ; a param or a
    // matching literal is fine. Correct scalar types pass too.
    assert_clean(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, count: int, title: text }
        shape D from Doc { title }
        mutation make(o: Org, t: text) -> D {
          create Doc { org = $o, count = 3, title = $t };
        }
        "#,
    );
}

#[test]
fn tx_step_ref_type_mismatch_rejected() {
    // `$batch.count` reads an int off the bound create; assigning it to a text column
    // is a family clash (`E0153`), typed through the binding reference.
    let (_, d) = analyze(
        r#"
        Batch { id: Id, count: int }
        Doc { id: Id, label: text, batch: Batch }
        shape D from Doc { label }
        mutation run(n: int) -> D {
          tx {
            create Batch { count = $n } as batch;
            create Doc { label = $batch.count, batch = $batch.id };
          }
        }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
}

// ---------- atomic update expressions --------------------------------------

#[test]
fn atomic_update_numeric_expr_is_clean() {
    // A self-referential arithmetic SET over numeric columns + a param.
    assert_clean(
        r#"
        Product { id: Id, qty: int, price: decimal(10, 2) }
        shape P from Product { qty, price }
        mutation adjust(id: Id, delta: int) -> P {
          update Product where (id = $id) { qty = qty + $delta };
        }
        mutation markup(id: Id, factor: decimal(10, 2)) -> P {
          update Product where (id = $id) { price = price * $factor };
        }
        "#,
    );
}

#[test]
fn atomic_update_precedence_and_parens_clean() {
    assert_clean(
        r#"
        Product { id: Id, qty: int, price: decimal(10, 2) }
        shape P from Product { qty, price }
        mutation recompute(id: Id, base: int, n: int) -> P {
          update Product where (id = $id) { qty = (qty + $base) * $n - 1 };
        }
        "#,
    );
}

#[test]
fn atomic_update_expr_in_create_rejected() {
    // A `create` has no existing row to self-reference — arithmetic is update-only.
    let (_, d) = analyze(
        r#"
        Product { id: Id, qty: int }
        shape P from Product { qty }
        mutation make(n: int) -> P { create Product { qty = qty + $n }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0230"]);
}

#[test]
fn atomic_update_nonnumeric_column_operand_rejected() {
    // A text column can't be an arithmetic operand.
    let (_, d) = analyze(
        r#"
        Product { id: Id, qty: int, name: text }
        shape P from Product { qty }
        mutation bump(id: Id, n: int) -> P {
          update Product where (id = $id) { qty = name + $n };
        }
        "#,
    );
    assert_eq!(errors(&d), ["E0231"]);
}

#[test]
fn atomic_update_nonnumeric_target_rejected() {
    // Assigning a numeric arithmetic result to a text column is the ordinary E0153.
    let (_, d) = analyze(
        r#"
        Product { id: Id, qty: int, name: text }
        shape P from Product { name }
        mutation bump(id: Id, n: int) -> P {
          update Product where (id = $id) { name = qty + $n };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0153"), "{:?}", codes(&d));
}

// ---------- filters --------------------------------------------------------

#[test]
fn filter_arity_mismatch() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, active: bool }
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
        Product { id: Id, name: text }
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
        Product { id: Id, name: text, active: bool, stock: int, @index(active, stock) }
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
        Product { id: Id, name: text, active: bool }
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
        City { id: Id, name: text }
        Address { id: Id, city: City }
        User { id: Id, name: text, address: Address }
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
        Product { id: Id, name: text, stock: int }
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
        Product { id: Id, name: text, active: bool, @index active }
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
        Product { id: Id, name: text (default bogus()) }
        "#,
    );
    assert_eq!(errors(&d), ["E0116"]);
}

// ---------- lints ----------------------------------------------------------

#[test]
fn nondeterministic_list_warns() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text }
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
        Product { id: Id, name: text }
        shape P from Product { name }
        query all() -> P[];
        "#,
    );
}

#[test]
fn bare_model_sort_term_defaults_to_asc() {
    // `@sort(name)` — no direction token. The canonical (fmt) spelling of an
    // ascending sort must register as one, not vanish as an unclassified arg.
    let (schema, d) = analyze(
        r#"
        @sort(name)
        Product { id: Id, name: text }
        shape P from Product { name }
        query all() -> P[];
        "#,
    );
    assert!(d.is_empty(), "{:?}", codes(&d));
    let product = &schema.models[0];
    assert_eq!(product.sort.len(), 1);
    assert_eq!(product.sort[0].path.segments[0].node, "name");
}

#[test]
fn raw_soft_delete_gap_warns() {
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Product { id: Id, deleted_at: timestamp?, name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (raw`name is not null`) order (name); }
        "#,
    );
    assert_eq!(codes(&d), ["W0102"]);
}

// ---------- duplicates -----------------------------------------------------

#[test]
fn duplicate_model_and_field() {
    let (_, d) = analyze("Doc { id: Id, name: text, name: int }\nDoc { id: Id, x: int }");
    assert!(errors(&d).contains(&"E0104"));
    assert!(errors(&d).contains(&"E0100"));
}

// ---------- operand typing --------------------------------------------------

#[test]
fn like_on_non_text_rejected() {
    // `~` needs a text column; `qty` is an int.
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, qty: int }
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
        Product { id: Id, name: text, @index name }
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
        Product { id: Id, name: text, maker: Maker }
        Maker { id: Id, name: text }
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
        Product { id: Id, name: text, qty: int }
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
        Product { id: Id, name: text, active: bool }
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
        Product { id: Id, name: text, maker: Maker, @index maker }
        Maker { id: Id, name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (maker = "0f-uuid") order (name); }
        "#,
    );
}

#[test]
fn column_vs_column_family_mismatch_rejected() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, qty: int }
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
        Product { id: Id, sku: text }
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
        Product { id: Id, name: text, maker: Maker, @index maker }
        Maker { id: Id, name: text }
        shape P from Product { name }
        query by_maker(maker: Maker) -> P[];
        "#,
    );
}

#[test]
fn relation_param_typed_wrong_model_rejected() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, maker: Maker }
        Maker { id: Id, name: text }
        Other { id: Id, name: text }
        shape P from Product { name }
        query by_maker(maker: Other) -> P[];
        "#,
    );
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn relation_param_typed_as_id_ok() {
    // a relation param may be typed as its key (`Id`) instead of the model.
    assert_clean(
        r#"
        @sort(name asc)
        Product { id: Id, name: text, maker: Maker, @index maker }
        Maker { id: Id, name: text }
        shape P from Product { name }
        query by_maker(maker: Id) -> P[];
        "#,
    );
}

// ---------- index inference + lints -----------------------------------------

#[test]
fn traversed_join_key_needs_index() {
    // The shape reaches `items.qty` (an inverse hop), so OrderItem's join key is
    // traversed with no covering `@index` — a hard error, no silent auto-index.
    let (_, d) = analyze(
        r#"
        @sort(placed_at desc)
        Order { id: Id, placed_at: timestamp, items: OrderItem[], @index placed_at }
        OrderItem { id: Id, order: Order, qty: int }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#,
    );
    assert_eq!(codes(&d), vec!["E0260"]);
    // The autofix inserts `@index order` on the model that owns the join key.
    let fix = d[0].fix.as_ref().expect("fix");
    assert_eq!(fix.model, "OrderItem");
    assert_eq!(fix.line, "@index order");
}

#[test]
fn traversed_join_key_satisfied_by_declared_index() {
    // The user declared the join-key index; the traversal is covered, and the
    // declared index counts as used (no W0104).
    assert_clean(
        r#"
        @sort(placed_at desc)
        Order { id: Id, placed_at: timestamp, items: OrderItem[], @index placed_at }
        OrderItem { id: Id, order: Order, qty: int, @index order }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#,
    );
}

#[test]
fn unindexed_query_errors() {
    // `by_status` filters `status`, but no index leads with it — the query scans.
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, status: text, @index name }
        shape P from Product { name }
        query by_status(status) -> P[] order (name);
        "#,
    );
    assert_eq!(codes(&d), vec!["E0260"]);
    let fix = d[0].fix.as_ref().expect("fix");
    assert_eq!(fix.model, "Product");
    assert_eq!(fix.line, "@index status");
}

#[test]
fn unindexed_satisfied_by_declared_index() {
    assert_clean(
        r#"
        Product { id: Id, name: text, status: text, @index status }
        shape P from Product { name }
        query by_status(status) -> P[] order (name);
        "#,
    );
}

#[test]
fn unindexed_satisfied_by_unsafe_annotation() {
    // The loud opt-out: greppable, silences the unindexed error, never silently dropped.
    assert_clean(
        r#"
        Product { id: Id, name: text, status: text, @index name }
        shape P from Product { name }
        query by_status(status) -> P[] order (name) unindexed(unsafe, "ops table, stays tiny");
        "#,
    );
}

#[test]
fn unindexed_satisfied_by_max_rows_in_block() {
    assert_clean(
        r#"
        Product { id: Id, name: text, status: text, @index name }
        shape P from Product { name }
        query by_status(s) -> P[] {
          list Product where (status = $s) order (name) unindexed(max_rows: 500);
        }
        "#,
    );
}

#[test]
fn mutation_where_unindexed_errors() {
    // A bulk `update` filtering a non-unique, unindexed column scans just like a
    // query would — E0260 (mutations carry no `unindexed(…)` clause to suppress it).
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, status: text }
        shape P from Product { name }
        mutation archive(s: text) -> P { update Product where (status = $s) { name = "x" }; }
        "#,
    );
    assert_eq!(codes(&d), vec!["E0260"]);
}

#[test]
fn mutation_where_keyed_on_unique_is_clean() {
    // The common case: a write keyed on `id` (unique) is served, no error.
    assert_clean(
        r#"
        Product { id: Id, name: text, status: text }
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
        Product { id: Id, name: text, status: text, @index status }
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
        Product { id: Id, sku: text (unique), name: text }
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
        Product { id: Id, name: text, price: int, @index price, @index name }
        shape P from Product { name }
        query all() -> P[];
        "#,
    );
    assert_eq!(codes(&d), vec!["W0104"]);
}

#[test]
fn paginated_sort_wants_an_index() {
    // No filter at all: a paginated list still pays for its sort — E0260 unless
    // the sort key is indexed.
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, created_at: timestamp }
        shape P from Product { name }
        query recent() -> P[] order (created_at desc) page (20);
        "#,
    );
    assert_eq!(codes(&d), vec!["E0260"]);
    assert_clean(
        r#"
        Product { id: Id, name: text, created_at: timestamp, @index created_at }
        shape P from Product { name }
        query recent() -> P[] order (created_at desc) page (20);
        "#,
    );
}

#[test]
fn unique_index_is_never_useless() {
    // A unique index is a constraint, not a perf structure — exempt from W0104
    // even with no queries at all.
    assert_clean("M { id: Id, a: text, b: text, @index(a, b) unique }");
}

#[test]
fn index_duplicating_unique_constraint_warns() {
    let (_, d) = analyze("Org { id: Id, slug: text (unique), @index slug }");
    assert_eq!(codes(&d), vec!["W0104"]);
}

#[test]
fn or_predicate_is_opaque_to_unindexed() {
    // First-column reasoning can't judge an `or`; the check stays silent rather than
    // guess (precision over recall).
    assert_clean(
        r#"
        Product { id: Id, name: text, a: text, b: text, @index name }
        shape P from Product { name }
        query q() -> P[] { list Product where (a = "x" or b = "y") order (name); }
        "#,
    );
}

// ---------- @no_id keyless legacy tables ----------------------------------

#[test]
fn no_id_suppresses_the_missing_id_error() {
    // A keyless legacy table opts out of the primary key with a reason; no E0261.
    assert_clean(
        r#"
        @no_id("legacy audit log has no surrogate key")
        Event { source: text (unique), payload: text }
        shape E from Event { source, payload }
        query event_by_source(source) -> E;
        "#,
    );
}

#[test]
fn no_id_requires_a_non_empty_reason() {
    let (_, d) = analyze(
        r#"
        @no_id
        Event { source: text (unique) }
        "#,
    );
    assert_eq!(errors(&d), ["E0262"]);
    let (_, d) = analyze(
        r#"
        @no_id("")
        Event { source: text (unique) }
        "#,
    );
    assert_eq!(errors(&d), ["E0262"]);
}

#[test]
fn keyless_get_must_key_on_a_unique_field() {
    // No `id` to key on: a `get` on a non-unique field is the ordinary E0144.
    let (_, d) = analyze(
        r#"
        @no_id("legacy")
        Event { source: text (unique), kind: text, @index kind }
        shape E from Event { kind }
        query by_kind(kind) -> E;
        "#,
    );
    assert_eq!(errors(&d), ["E0144"]);
}

#[test]
fn keyless_keyset_page_needs_a_unique_sort_key() {
    // No `id` tiebreaker → a keyset page must sort on a unique column, else E0263.
    let (_, d) = analyze(
        r#"
        @no_id("legacy")
        Event { source: text (unique), at: timestamp, @index at }
        shape E from Event { source }
        query recent() -> E[] { list Event order (at desc) page (20); }
        "#,
    );
    assert_eq!(errors(&d), ["E0263"]);
    // A unique sort key is deterministic; and an offset page needs no tiebreaker.
    assert_clean(
        r#"
        @no_id("legacy")
        Event { source: text (unique), at: timestamp, @index at }
        shape E from Event { source }
        query by_source() -> E[] { list Event order (source) page (20); }
        query offset_page() -> E[] { list Event order (at desc) page (20) offset; }
        "#,
    );
}

#[test]
fn keyless_create_must_set_a_unique_read_back_key() {
    // A declared-shape create on a keyless model needs a unique column to read back by.
    let (_, d) = analyze(
        r#"
        @no_id("legacy")
        Event { source: text? (unique), payload: text }
        shape E from Event { source, payload }
        mutation record(p: text) -> E { create Event { payload = $p }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0264"]);
    // Setting the unique column keys the read-back.
    assert_clean(
        r#"
        @no_id("legacy")
        Event { source: text (unique), payload: text }
        shape E from Event { source, payload }
        mutation record(s: text, p: text) -> E { create Event { source = $s, payload = $p }; }
        "#,
    );
}

#[test]
fn forward_relation_to_a_keyless_model_errors() {
    // A keyless model has no `id` for an FK to reference (E0265).
    let (_, d) = analyze(
        r#"
        @no_id("legacy")
        Event { source: text (unique) }
        Log { id: Id, event: Event, note: text }
        "#,
    );
    assert_eq!(errors(&d), ["E0265"]);
}

#[test]
fn scope_filter_counts_toward_pattern() {
    // `@scope` is injected into every query on the model, so its
    // columns are part of every query's index pattern.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert_eq!(codes(&d), vec!["E0260"]);
    assert_clean(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
}

// ---------- named scope: decl / @scope / scoped  ------------------

#[test]
fn scope_term_must_bind_ctx_field() {
    // A `scope` decl term binds `col: Type = $ctx.<field>`; a non-`$ctx` binding is
    // `E0180` (the predicate-form rule, now at the decl site).
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $other.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0180"), "{:?}", codes(&d));

    // A multi-segment `$ctx` path is not the flat scope-field form → `E0180`.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org.id)
        @scope Tenant
        Doc { id: Id, org: Org, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0180"), "{:?}", codes(&d));
}

#[test]
fn multi_term_ctx_equality_scope_is_clean() {
    // A conjunction of `col = $ctx.field` equalities is the allowed shape (one decl
    // with two terms).
    assert_clean(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org, region: Region = $ctx.region)
        @scope Tenant
        Doc { id: Id, org: Org, region: Region, title: text, @index(org, region) }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        @scope Nope
        Doc { id: Id, org: Org, title: text }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, title: text }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0184"), "{:?}", codes(&d));

    // A column of the wrong type also fails E0184 (`org` is text, not the relation `Org`).
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: text, title: text }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Other (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] scoped Other order (title);
        "#,
    );
    assert!(errors(&d).contains(&"E0185"), "{:?}", codes(&d));
}

#[test]
fn nest_only_scoped_child_is_touched_e0185() {
    // A scoped child reached *only* through a nested shape sub-object counts as touched
    // (its `@scope` is injected into the nest join/subquery), so the callable must
    // satisfy it. Here `LineItem` is `@scope Region`, reached only via `items { … }`;
    // `scoped Tenant` covers the root `Order` but not `LineItem`'s Region axis → `E0185`.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        Order { id: Id, org: Org, total: int, items: LineItem[], @index org }
        @scope Region
        LineItem { id: Id, order: Order, region: Region, sku: text, @index region }
        shape OrderCard from Order { total, items { sku } }
        query order_by_id(id) -> OrderCard scoped Tenant;
        "#,
    );
    assert!(errors(&d).contains(&"E0185"), "{:?}", codes(&d));

    // Naming both axes satisfies every touched model's alternative → checks clean.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        Order { id: Id, org: Org, total: int, items: LineItem[], @index org }
        @scope Region
        LineItem { id: Id, order: Order, region: Region, sku: text, @index region, @index order }
        shape OrderCard from Order { total, items { sku } }
        query order_by_id(id) -> OrderCard scoped Tenant, Region;
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn scope_field_type_sourced_from_the_decl() {
    // The scope field's `$ctx` type comes from the decl (`org: Org`), so a scoped query
    // requires `$ctx.org` typed as an `Org` relation — no per-callable inference needed.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text }
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
        Doc { id: Id, title: text, @index title }
        shape D from Doc { title }
        query all() -> D[] unscoped("nothing to opt out of") order (title);
        "#,
    );
    assert_eq!(codes(&d), vec!["W0106"]);
}

// ---------- $ctx inference + coherence  -----------------------------

#[test]
fn ctx_inferred_from_use_is_clean() {
    // No declaration anywhere: `$ctx.org`'s type is inferred from the `org` column
    // it compares against.
    assert_clean(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
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
    // no `where` of its own.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
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
    // a query on an *unscoped* model that reaches a *scoped* model through a
    // shape relation joins it, and codegen injects the joined model's `@scope` into
    // the join `ON` — so the callable must require that model's `$ctx.org`, else the
    // injected `:ctx_org` bind is unbound at runtime.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
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
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text, @index name }
        Ticket { id: Id, raised_by: Contact, subject: text }
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
    // `unscoped`  drops all scope handling, joins included, so the joined
    // scoped model contributes no `$ctx` requirement (mirrors codegen).
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org, @index title }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org, @index title }
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
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text }
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
    // a mutation's declared-shape re-select  projects the return shape, so a
    // relation reach in that shape joins a scoped model and injects its `@scope` — the
    // mutation must require the joined model's `$ctx.org` too. Here `Ticket` is
    // unscoped but its re-select reaches the org-scoped `Contact`.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
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

// ---------- tx step bindings (`create … as name` / `$name.field`) -----------

#[test]
fn tx_step_ref_to_prior_create_is_clean() {
    assert_clean(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#,
    );
}

#[test]
fn tx_step_ref_reaches_any_prior_step_is_clean() {
    // A 3-step tx where step 3 references step 1 — the case `^` could not express.
    assert_clean(
        r#"
        Org { id: Id, name: text }
        User { id: Id, org: Org, email: text }
        Log { id: Id, org: Org, actor: User }
        shape OrgCard from Org { name }
        mutation onboard(name: text, email: text) -> OrgCard {
          tx {
            create Org { name = $name } as org;
            create User { org = $org.id, email = $email } as user;
            create Log { org = $org.id, actor = $user.id };
          }
        }
        "#,
    );
}

#[test]
fn tx_step_ref_to_unknown_field_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.nope, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0111"), "{:?}", codes(&d));
}

#[test]
fn tx_step_binding_shadowing_a_param_rejected() {
    // A binding may not shadow a param — `$user` must name one thing (E0280).
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(user: text, city: text) -> UserCard {
          tx {
            create User { email = $user } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0280"), "{:?}", codes(&d));
}

#[test]
fn tx_duplicate_step_binding_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        shape UserCard from User { email }
        mutation twins(a: text, b: text) -> UserCard {
          tx {
            create User { email = $a } as u;
            create User { email = $b } as u;
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0280"), "{:?}", codes(&d));
}

#[test]
fn tx_forward_reference_rejected() {
    // `$later` is bound by a *later* step — a binding reaches only prior steps (E0281).
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape A from Address { city }
        mutation m(email: text, city: text) -> A {
          tx {
            create Address { user = $later.id, city = $city };
            create User { email = $email } as later;
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0281"), "{:?}", codes(&d));
}

#[test]
fn tx_unbound_name_rejected() {
    // `$nope` is neither a param nor a bound step (E0281).
    let (_, d) = analyze(
        r#"
        Address { id: Id, city: text, ref_id: text }
        shape A from Address { city }
        mutation m(city: text) -> A {
          tx {
            create Address { ref_id = $nope.id, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0281"), "{:?}", codes(&d));
}

#[test]
fn step_ref_outside_tx_rejected() {
    // A step reference in a plain (non-tx) create has no binding in scope (E0281).
    let (_, d) = analyze(
        r#"
        Address { id: Id, city: text, ref_id: text }
        shape A from Address { city }
        mutation m(city: text) -> A {
          create Address { ref_id = $x.id, city = $city };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0281"), "{:?}", codes(&d));
}

// ---------- custom `on:` joins ----------------------------------------------

#[test]
fn custom_join_resolves_clean() {
    // A legacy-key join: both sides are table-qualified columns that resolve
    // against the FK-holding model (`order`) and its target (`user`).
    assert_clean(
        r#"
        Order {
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = user.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
}

#[test]
fn custom_join_unknown_column_rejected() {
    // `user.nope` is not a column on the target model.
    let (_, d) = analyze(
        r#"
        Order {
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = user.nope)
        }
        User { id: Id, name: text, legacy_id: int }
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
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = customers.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
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
          id: Id
          user_ref: int
          placed_by: User (on: user_ref = user.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
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
          id: Id
          user_ref: int (on: order.user_ref = user.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
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
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = $x)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0126"), "{:?}", codes(&d));
}

// ---------- multi-scope DNF: alternatives, E0185, E0186  --------------

#[test]
fn or_model_query_injects_only_its_named_alternative() {
    // A model with two stacked `@scope` decorators is an OR of alternatives; each
    // query names one and injects only that axis.
    let (schema, diags) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
          @index author
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
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        @sort(created desc)
        Comment {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
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
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        @sort(created desc)
        Comment {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
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
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        Comment {
          id: Id
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
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        Comment {
          id: Id
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

#[test]
fn or_model_ctx_follows_the_chosen_alternative() {
    // The `$ctx` requirement derives from the alternative the callable *chose*,
    // not the model's first `@scope` line — sema's ctx bag must carry exactly the
    // `:ctx_<field>` binds codegen injects.
    let (schema, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
          @index author
        }
        shape PostCard from Post { body }
        query my_posts() -> PostCard[] scoped Author { list Post order (created desc); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "my_posts")
        .unwrap();
    assert_eq!(q.ctx_requires.len(), 1, "{:?}", q.ctx_requires);
    assert_eq!(q.ctx_requires[0].field, "user");
}

#[test]
fn or_model_create_naming_both_alternatives_is_clean() {
    // A create naming several alternatives auto-sets every named axis's column;
    // none of them is "missing" (E0146), and the ctx bag carries both fields.
    let (schema, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp (default now())
          @index page
          @index author
        }
        shape PostCard from Post { body }
        query on_page() -> PostCard[] scoped Page   { list Post; }
        query mine()    -> PostCard[] scoped Author { list Post; }
        mutation write_post(body: text) -> PostCard scoped Page, Author {
          create Post { body = $body };
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let m = schema
        .mutations
        .iter()
        .find(|m| m.name == "write_post")
        .unwrap();
    let mut fields: Vec<&str> = m.ctx_requires.iter().map(|c| c.field.as_str()).collect();
    fields.sort_unstable();
    assert_eq!(fields, ["page", "user"]);
}

#[test]
fn scoped_create_assigning_an_unchosen_alternative_column_is_e0181() {
    // Every alternative's column is engine-domain on a scoped create — assigning
    // one the callable didn't choose is still planting the row into a scope.
    let (_, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(body asc)
        Post {
          id: Id
          page:   Page
          author: User?
          body:   text
          @index page
        }
        shape PostCard from Post { body }
        query on_page() -> PostCard[] scoped Page { list Post; }
        mutation write_post(body: text, author: Id) -> PostCard scoped Page {
          create Post { body = $body, author = $author };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0181"), "{:?}", codes(&d));
}

#[test]
fn unindexed_annotation_on_an_unscoped_query_is_not_stale() {
    // An `unscoped` query injects no scope, so a scope column's index cannot make
    // its annotation "stale" — the pattern is the query's own filter only.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        @sort(created desc)
        Doc {
          id: Id
          org:     Org
          title:   text
          created: timestamp
          @index(org, title)
        }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant { list Doc; }
        query export(since: timestamp > created) -> D[]
          unscoped("audit: whole-corpus export")
          unindexed(unsafe, "deliberate full scan");
        "#,
    );
    assert!(
        !codes(&d).contains(&"W0105"),
        "annotation wrongly stale: {:?}",
        codes(&d)
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

// ---------- @was rename directive -------------------------------------------

#[test]
fn field_was_rename_is_clean() {
    // `@was("upc")` on a field whose old column is gone from the model — a valid rename.
    assert_clean(
        r#"
        @sort(name asc)
        Product {
          id: Id
          name: text
          barcode: text? @was("upc")
        }
        shape ProductCard from Product { name }
        query products() -> ProductCard[];
        "#,
    );
}

#[test]
fn model_was_rename_is_clean() {
    assert_clean(
        r#"
        @sort(name asc)
        @was("legacy_product")
        Product {
          id: Id
          name: text
        }
        shape ProductCard from Product { name }
        query products() -> ProductCard[];
        "#,
    );
}

#[test]
fn field_was_naming_a_live_column_is_e0191() {
    // `barcode @was("sku")` while `sku` still exists — can't be the rename source.
    let (_, d) = analyze(
        r#"
        Product {
          id: Id
          sku: text
          barcode: text? @was("sku")
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0191"), "{:?}", codes(&d));
}

#[test]
fn field_was_naming_itself_is_e0190() {
    let (_, d) = analyze(
        r#"
        Product {
          id: Id
          barcode: text? @was("barcode")
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0190"), "{:?}", codes(&d));
}

#[test]
fn model_was_naming_a_live_table_is_e0191() {
    let (_, d) = analyze(
        r#"
        Legacy { id: Id, name: text }
        @was("legacy")
        Product { id: Id, name: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0191"), "{:?}", codes(&d));
}

#[test]
fn model_was_naming_its_own_table_is_e0190() {
    let (_, d) = analyze(
        r#"
        @was("product")
        Product { id: Id, name: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0190"), "{:?}", codes(&d));
}

// ---------- enums ----------------------------------------------------------

#[test]
fn enum_typed_field_is_a_scalar_not_a_relation() {
    let (schema, d) = analyze(
        r#"
        enum Status { pending, paid, shipped }
        Order { id: Id, status: Status, total: int }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let order = schema.model("Order").expect("Order model");
    match &order.member("status").expect("status member").kind {
        MemberKind::Scalar { enum_name, .. } => {
            assert_eq!(enum_name.as_deref(), Some("Status"));
        }
        other => panic!("status should be a scalar enum column, got {other:?}"),
    }
    assert!(schema
        .enum_("Status")
        .expect("enum resolved")
        .has_variant("paid"));
}

#[test]
fn enum_default_must_be_a_member() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status (default shipped), total: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0155"), "{:?}", codes(&d));
}

#[test]
fn enum_default_member_is_clean() {
    assert_clean(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status (default pending), total: int }
        "#,
    );
}

#[test]
fn bare_default_on_non_enum_column_is_e0155() {
    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int (default whoops) }
        "#,
    );
    assert!(errors(&d).contains(&"E0155"), "{:?}", codes(&d));
}

#[test]
fn where_on_non_member_variant_is_e0154() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query paid_orders() -> OrderRow[] { list Order where (status = shipped); }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}

#[test]
fn where_on_member_variant_has_no_errors() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int, @index status }
        shape OrderRow from Order { status, total }
        query paid_orders() -> OrderRow[] { list Order where (status = paid) order (total); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn in_list_on_enum_column_checks_each_variant() {
    // Members + a `$param` element are clean; one borrowed variant is E0154.
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid, shipped }
        Order { id: Id, status: Status, total: int, @index status }
        shape OrderRow from Order { status, total }
        query open(extra: Status) -> OrderRow[] {
          list Order where (status in (pending, paid, $extra)) order (total);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));

    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        enum Priority { low = 1, high = 2 }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query open() -> OrderRow[] { list Order where (status in (pending, low)); }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}

#[test]
fn in_list_elements_are_family_checked() {
    // A text element in a numeric column's list is the same E0151 an `=` gives.
    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int, note: text }
        shape OrderRow from Order { total }
        query cheap() -> OrderRow[] { list Order where (total in (1, 2, "three")); }
        "#,
    );
    assert!(errors(&d).contains(&"E0151"), "{:?}", codes(&d));

    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int, note: text, @index total }
        shape OrderRow from Order { total }
        query cheap(x: int) -> OrderRow[] { list Order where (total in (1, 2, $x)); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn in_list_with_unknown_param_is_reported() {
    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int }
        shape OrderRow from Order { total }
        query cheap() -> OrderRow[] { list Order where (total in (1, $nope)); }
        "#,
    );
    assert!(errors(&d).contains(&"E0113"), "{:?}", codes(&d));
}

#[test]
fn create_with_non_member_variant_is_e0154() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        mutation place() -> OrderRow { create Order { status = shipped, total = 1 } }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}

#[test]
fn create_with_member_variant_has_no_errors() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        mutation place() -> OrderRow { create Order { status = paid, total = 1 } }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn enum_name_colliding_with_a_model_is_e0106() {
    let (_, d) = analyze(
        r#"
        Status { id: Id, name: text }
        enum Status { pending, paid }
        "#,
    );
    assert!(errors(&d).contains(&"E0106"), "{:?}", codes(&d));
}

#[test]
fn string_enum_with_explicit_value_resolves() {
    let (schema, d) = analyze(
        r#"
        enum Status { pending, paid = "PAID", shipped }
        Order { id: Id, status: Status, total: int }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let en = schema.enum_("Status").expect("enum resolved");
    assert!(matches!(en.kind, based_sema::EnumKind::Str));
    assert_eq!(
        en.wire_of("paid"),
        Some(&based_sema::EnumValue::Str("PAID".into()))
    );
    assert_eq!(
        en.wire_of("pending"),
        Some(&based_sema::EnumValue::Str("pending".into()))
    );
    // The stored column is text for a string enum.
    match &schema
        .model("Order")
        .unwrap()
        .member("status")
        .unwrap()
        .kind
    {
        MemberKind::Scalar { ty, .. } => assert_eq!(*ty, based_ast::Primitive::Text),
        other => panic!("{other:?}"),
    }
}

#[test]
fn int_enum_resolves_and_is_an_integer_column() {
    let (schema, d) = analyze(
        r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, priority: Priority (default low), title: text }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let en = schema.enum_("Priority").expect("enum resolved");
    assert!(en.is_int());
    assert_eq!(en.wire_of("high"), Some(&based_sema::EnumValue::Int(2)));
    match &schema
        .model("Ticket")
        .unwrap()
        .member("priority")
        .unwrap()
        .kind
    {
        MemberKind::Scalar { ty, enum_name, .. } => {
            assert_eq!(*ty, based_ast::Primitive::Int);
            assert_eq!(enum_name.as_deref(), Some("Priority"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn mixed_int_and_bare_variants_is_e0156() {
    let (_, d) = analyze(
        r#"
        enum Bad { low = 0, medium, high = 2 }
        Ticket { id: Id, priority: Bad }
        "#,
    );
    assert!(errors(&d).contains(&"E0156"), "{:?}", codes(&d));
}

#[test]
fn mixed_int_and_string_variants_is_e0156() {
    let (_, d) = analyze(
        r#"
        enum Bad { low = 0, medium = "MID" }
        Ticket { id: Id, priority: Bad }
        "#,
    );
    assert!(errors(&d).contains(&"E0156"), "{:?}", codes(&d));
}

#[test]
fn duplicate_variant_value_is_e0157() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid = "pending" }
        Order { id: Id, status: Status }
        "#,
    );
    assert!(errors(&d).contains(&"E0157"), "{:?}", codes(&d));
}

#[test]
fn duplicate_int_variant_value_is_e0157() {
    let (_, d) = analyze(
        r#"
        enum Priority { low = 0, medium = 0 }
        Ticket { id: Id, priority: Priority }
        "#,
    );
    assert!(errors(&d).contains(&"E0157"), "{:?}", codes(&d));
}

#[test]
fn duplicate_variant_name_is_e0104() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, pending }
        Order { id: Id, status: Status }
        "#,
    );
    assert!(errors(&d).contains(&"E0104"), "{:?}", codes(&d));
}

#[test]
fn ordered_op_on_string_enum_is_e0158() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query high() -> OrderRow[] { list Order where (status > pending); }
        "#,
    );
    assert!(errors(&d).contains(&"E0158"), "{:?}", codes(&d));
}

#[test]
fn ordered_op_on_int_enum_is_clean() {
    let (_, d) = analyze(
        r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, priority: Priority, title: text, @index priority }
        shape TicketRow from Ticket { priority, title }
        query urgent() -> TicketRow[] { list Ticket where (priority >= medium) order (title); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    assert!(!codes(&d).contains(&"E0158"), "{:?}", codes(&d));
}

#[test]
fn int_enum_variant_membership_still_checked_by_name() {
    let (_, d) = analyze(
        r#"
        enum Priority { low = 0, high = 1 }
        Ticket { id: Id, priority: Priority, title: text }
        shape TicketRow from Ticket { priority, title }
        query q() -> TicketRow[] { list Ticket where (priority = urgent); }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}

// ---------- decimal / float (D83) ------------------------------------------

#[test]
fn decimal_and_float_columns_resolve() {
    let (schema, d) = analyze(
        r#"
        Ledger {
          id: Id
          price: decimal(12, 2)
          bare:  decimal
          score: float
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let m = schema.model("Ledger").unwrap();
    for f in ["price", "bare", "score"] {
        assert!(matches!(
            m.member(f).unwrap().kind,
            MemberKind::Scalar { .. }
        ));
    }
}

#[test]
fn bad_decimal_precision_scale_errors() {
    // scale > precision
    let (_, d) = analyze("Ledger { id: Id, price: decimal(2, 5) }");
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
    // precision over the 38 cap
    let (_, d) = analyze("Ledger { id: Id, price: decimal(40, 2) }");
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
    // scale zero (need 1 <= scale)
    let (_, d) = analyze("Ledger { id: Id, price: decimal(10, 0) }");
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
}

#[test]
fn numeric_literal_binds_to_decimal_and_float_columns() {
    let (_, d) = analyze(
        r#"
        Ledger { id: Id, price: decimal(12, 2), score: float, name: text }
        shape LedgerRow from Ledger { price, score }
        query dear() -> LedgerRow[] { list Ledger where (price > 9.99) unindexed(unsafe) order (name); }
        query fast() -> LedgerRow[] { list Ledger where (score >= 0.5) unindexed(unsafe) order (name); }
        "#,
    );
    // A numeric literal is compatible with a decimal/float column (no E0150/E0151).
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn ordered_compare_on_decimal_allowed() {
    let (_, d) = analyze(
        r#"
        Ledger { id: Id, price: decimal(12, 2), name: text }
        shape LedgerRow from Ledger { price }
        query pricey(min: decimal(12, 2) >= price) -> LedgerRow[] { list Ledger unindexed(unsafe) order (name); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn decimal_default_must_be_a_decimal_literal() {
    let (_, d) = analyze(r#"Ledger { price: decimal(12, 2) (default "x") }"#);
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
    // an integer or a fractional literal is fine
    assert_clean(
        "Ledger { id: Id, a: decimal(12, 2) (default 5), b: decimal(12, 2) (default 9.99) }",
    );
}

// ---------- whole-query raw bodies (raw.md's third level) -------------------

#[test]
fn raw_query_body_with_typed_params_is_clean() {
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        User { id: Id, org: Org, name: text, email: text }
        shape UserRow from User { name, email }
        query heavy_users(min: int) -> UserRow[] {
          raw`SELECT u.name AS name, u.email AS email FROM user u WHERE u.id >= ${min}`;
        }
        "#,
    );
    assert!(d.is_empty(), "{:?}", codes(&d));
    let q = &schema.queries[0];
    assert_eq!(q.target, "User");
    assert_eq!(q.verb, Verb::List);
    assert!(q.many && !q.paginated);
}

#[test]
fn raw_query_scalar_return_needs_no_unique_key() {
    // A raw `get` skips E0144 — the SQL owns its keying.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query one(who: text) -> UserRow { raw`SELECT name FROM user WHERE name = ${who}`; }
        "#,
    );
    assert!(d.is_empty(), "{:?}", codes(&d));
}

#[test]
fn raw_query_param_must_be_typed() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query heavy(min) -> UserRow[] { raw`SELECT name FROM user WHERE id >= ${min}`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0210"), "{:?}", codes(&d));
}

#[test]
fn raw_query_param_binding_is_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text, created_at: timestamp }
        shape UserRow from User { name }
        query heavy(since: timestamp > created_at) -> UserRow[] { raw`SELECT name FROM user`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0210"), "{:?}", codes(&d));
}

#[test]
fn raw_query_unknown_param_is_reported() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query heavy() -> UserRow[] { raw`SELECT name FROM user WHERE id = ${nope}`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0113"), "{:?}", codes(&d));
}

#[test]
fn raw_query_ctx_ref_is_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query mine() -> UserRow[] { raw`SELECT name FROM user WHERE org = ${ctx.org}`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0214"), "{:?}", codes(&d));
}

#[test]
fn raw_query_cannot_stream() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query all() -> stream UserRow { raw`SELECT name FROM user`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0212"), "{:?}", codes(&d));
}

#[test]
fn raw_query_on_scoped_model_must_be_unscoped() {
    // `scoped` promises an injection the engine can't perform in raw SQL (E0211);
    // `unscoped("…")` is the loud, legal spelling.
    let src_scoped = r#"
        scope Tenant (org: Org = $ctx.org)
        Org { id: Id, name: text }
        @scope Tenant
        Ticket { id: Id, org: Org, title: text }
        shape TicketRow from Ticket { title }
        query all() -> TicketRow[] scoped Tenant { raw`SELECT title FROM ticket`; }
    "#;
    let (_, d) = analyze(src_scoped);
    assert!(errors(&d).contains(&"E0211"), "{:?}", codes(&d));

    let src_unscoped = r#"
        scope Tenant (org: Org = $ctx.org)
        Org { id: Id, name: text }
        @scope Tenant
        Ticket { id: Id, org: Org, title: text }
        shape TicketRow from Ticket { title }
        query all() -> TicketRow[] unscoped("admin report") { raw`SELECT title FROM ticket`; }
    "#;
    let (_, d) = analyze(src_unscoped);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));

    // Writing neither is the ordinary missing-ack error.
    let src_bare = r#"
        scope Tenant (org: Org = $ctx.org)
        Org { id: Id, name: text }
        @scope Tenant
        Ticket { id: Id, org: Org, title: text }
        shape TicketRow from Ticket { title }
        query all() -> TicketRow[] { raw`SELECT title FROM ticket`; }
    "#;
    let (_, d) = analyze(src_bare);
    assert!(errors(&d).contains(&"E0182"), "{:?}", codes(&d));
}

#[test]
fn raw_query_shape_must_be_flat() {
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        User { id: Id, org: Org, name: text }
        shape UserCard from User { name, org { name } }
        query all() -> UserCard[] { raw`SELECT name FROM user`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0213"), "{:?}", codes(&d));
}

#[test]
fn raw_query_soft_delete_gap_is_linted() {
    // The target model's tombstone, and any other soft-delete table the raw text
    // mentions, each get a W0102 — never silent.
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp?, user: User, total: int }
        shape UserRow from User { name }
        query buyers() -> UserRow[] {
          raw`SELECT u.name AS name FROM user u JOIN order o ON o.user_id = u.id WHERE u.deleted_at IS NULL`;
        }
        "#,
    );
    let warns = d
        .iter()
        .filter(|x| x.severity == Severity::Warning && x.code == "W0102")
        .count();
    assert_eq!(warns, 2, "{:?}", codes(&d));
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

// ---------- `-> ok`: the destructive-mutation acknowledgement ----------------

#[test]
fn ack_hard_delete_resolves_the_deleted_model() {
    let (schema, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Comment { id: Id, deleted_at: timestamp?, body: text }
        mutation purge_comment(id: Id) -> ok {
          hard delete Comment where (id = $id);
        }
        "#,
    );
    assert!(d.is_empty(), "{:?}", codes(&d));
    let m = &schema.mutations[0];
    assert!(m.ack);
    assert_eq!(m.ret_model, "Comment");
    assert_eq!(m.ret_shape, None);
}

#[test]
fn ack_plain_delete_on_plain_model_is_clean() {
    let (schema, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        mutation drop_tag(id: Id) -> ok {
          delete Tag where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    assert!(schema.mutations[0].ack);
}

#[test]
fn shape_on_real_delete_is_rejected() {
    // No surviving row to read back as the shape → E0220.
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        shape TagCard from Tag { label }
        mutation drop_tag(id: Id) -> TagCard {
          delete Tag where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0220"), "{:?}", codes(&d));
}

#[test]
fn shape_on_hard_delete_is_rejected() {
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Comment { id: Id, deleted_at: timestamp?, body: text }
        shape CommentRow from Comment { body }
        mutation purge_comment(id: Id) -> CommentRow {
          hard delete Comment where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0220"), "{:?}", codes(&d));
}

#[test]
fn shape_survives_when_a_tx_sibling_creates_the_return_row() {
    // The delete removes one model's row, but the return model is re-created by a
    // sibling write — a surviving row exists, so the declared shape stands.
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        Audit { id: Id, note: text }
        shape AuditRow from Audit { note }
        mutation drop_tag(id: Id, note: text) -> AuditRow {
          tx {
            delete Tag where (id = $id);
            create Audit { note = $note };
          }
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn ack_on_soft_delete_is_rejected() {
    // A plain `delete` on a soft-delete model tombstones — the row survives (E0221).
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Comment { id: Id, deleted_at: timestamp?, body: text }
        mutation remove_comment(id: Id) -> ok {
          delete Comment where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0221"), "{:?}", codes(&d));
}

#[test]
fn ack_on_create_or_update_is_rejected() {
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        mutation rename_tag(id: Id, label: text) -> ok {
          update Tag where (id = $id) { label = $label };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0221"), "{:?}", codes(&d));
}

#[test]
fn ack_without_a_real_delete_is_rejected() {
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        mutation noop() -> ok {
          raw`ANALYZE`;
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0221"), "{:?}", codes(&d));
}

#[test]
fn ack_on_a_query_is_rejected() {
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        query tags() -> ok;
        "#,
    );
    assert!(errors(&d).contains(&"E0222"), "{:?}", codes(&d));
}

#[test]
fn ack_scoped_hard_delete_keeps_scope_ack_checking() {
    // The ack mutation still owes the scope acknowledgement for the model it deletes.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Comment { id: Id, org: Org, body: text }
        mutation purge_comment(id: Id) -> ok {
          hard delete Comment where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0182"), "{:?}", codes(&d));
}

// ---------- aggregations + group by + having (T4) --------------------------

const AGG_MODELS: &str = r#"
        Buyer { id: Id, name: text }
        @soft_delete(deleted_at)
        Order {
          id: Id
          deleted_at: timestamp?
          buyer: Buyer
          total: decimal(12, 2)
          qty: int
          note: text
        }
"#;

#[test]
fn aggregate_query_group_by_is_clean() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape BuyerStats from Order {
          who = buyer
          orders = count()
          revenue = sum(total)
          avg_qty = avg(qty)
          biggest = max(total)
        }
        query buyer_stats() -> BuyerStats[] {
          list Order group by (buyer) having (revenue > 100) order (revenue desc);
        }
        "#,
    );
    assert_clean(&src);
}

#[test]
fn global_aggregate_get_is_clean() {
    // No group by, all-aggregate shape → one whole-table row (a `get`), not
    // flagged as an unkeyed get.
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape OrderTotals from Order { orders = count(), revenue = sum(total) }
        query order_totals() -> OrderTotals { get Order; }
        "#,
    );
    assert_clean(&src);
}

#[test]
fn sum_of_text_column_is_e0241() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, x = sum(note) }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0241"), "{:?}", codes(&d));
}

#[test]
fn count_with_argument_is_e0240() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, n = count(total) }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0240"), "{:?}", codes(&d));
}

#[test]
fn unknown_aggregate_is_e0240() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, n = median(total) }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0240"), "{:?}", codes(&d));
}

#[test]
fn ungrouped_projected_column_is_e0242() {
    // `note` is projected but not aggregated and not in `group by`.
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, note, orders = count() }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0242"), "{:?}", codes(&d));
}

#[test]
fn having_reference_not_projected_is_e0242() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Stats from Order { who = buyer, orders = count() }
        query bad() -> Stats[] { list Order group by (buyer) having (revenue > 10); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0242"), "{:?}", codes(&d));
}

#[test]
fn group_by_on_non_aggregate_query_is_e0243() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Plain from Order { note }
        query bad() -> Plain[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0243"), "{:?}", codes(&d));
}

#[test]
fn page_on_aggregate_query_is_e0244() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Stats from Order { who = buyer, orders = count() }
        query bad() -> Stats[] { list Order group by (buyer) page (20); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0244"), "{:?}", codes(&d));
}

#[test]
fn aggregate_shape_nested_is_e0245() {
    // An aggregate shape must be flat.
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { orders = count(), buyer { name } }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0245"), "{:?}", codes(&d));
}

#[test]
fn aggregate_shape_as_mutation_return_is_e0245() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Stats from Order { orders = count() }
        mutation touch(id: Id) -> Stats { update Order where (id = $id) { qty = 1 }; }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0245"), "{:?}", codes(&d));
}

// ---------- upsert (`create … on conflict update`) -------------------------

#[test]
fn upsert_unique_target_is_clean() {
    assert_clean(
        r#"
        Page { id: Id, path: text (unique), hits: int }
        shape PageRow from Page { path, hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
        }
        "#,
    );
}

#[test]
fn upsert_composite_unique_index_is_clean() {
    assert_clean(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, slug: text, views: int, @index (org, slug) unique }
        shape DocRow from Doc { slug, views }
        mutation touch(slug: text) -> DocRow scoped Tenant {
          create Doc { slug = $slug, views = 1 } on conflict (org, slug) update { views = views + 1 };
        }
        "#,
    );
}

#[test]
fn upsert_non_unique_target_is_e0250() {
    let (_, d) = analyze(
        r#"
        Page { id: Id, path: text, hits: int }
        shape PageRow from Page { path, hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0250"), "{:?}", codes(&d));
}

#[test]
fn upsert_update_branch_sets_conflict_col_is_e0251() {
    let (_, d) = analyze(
        r#"
        Page { id: Id, path: text (unique), hits: int }
        shape PageRow from Page { path, hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { path = $path };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0251"), "{:?}", codes(&d));
}

#[test]
fn upsert_target_not_set_by_create_is_e0252() {
    // The conflict is on `code`, a unique column the create never assigns (so there is no
    // value to conflict on or read the winning row back by).
    let (_, d) = analyze(
        r#"
        Page { id: Id, path: text (unique), code: text (unique), hits: int (default 0) }
        shape PageRow from Page { hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path } on conflict (code) update { hits = hits + 1 }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0252"), "{:?}", codes(&d));
}

#[test]
fn upsert_on_soft_delete_model_is_e0253() {
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Page { id: Id, deleted_at: timestamp?, path: text (unique), hits: int }
        shape PageRow from Page { path, hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0253"), "{:?}", codes(&d));
}

#[test]
fn upsert_scoped_target_omits_scope_col_is_e0254() {
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, slug: text (unique), views: int }
        shape DocRow from Doc { slug, views }
        mutation touch(slug: text) -> DocRow scoped Tenant {
          create Doc { slug = $slug, views = 1 } on conflict (slug) update { views = views + 1 };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0254"), "{:?}", codes(&d));
}

// ---------- opaque `raw(…)` columns + exotic indexes -----------------------

const PLACE: &str = r#"
    Place {
      id:       Id
      name:     text
      location: raw("geometry(Point,4326)")?
      tags:     raw({ postgres: "tsvector", mariadb: "text", sqlite: "text" })?
      @index name
      @index location using gist
      @index raw("(lower(name))")
    }

    shape PlaceRow from Place {
      id
      name
      location
      area = raw`ST_Area(location)`
    }

    query place(id) -> PlaceRow;
    query by_name(name) -> PlaceRow[] order (name);
    "#;

#[test]
fn opaque_column_projects_and_indexes_clean() {
    assert_clean(PLACE);
    let (schema, _) = analyze(PLACE);
    let place = schema.model("Place").unwrap();
    let loc = place.member("location").unwrap();
    let spec = loc.kind.opaque().expect("opaque column");
    assert_eq!(spec.for_dialect("mariadb"), Some("geometry(Point,4326)"));
    // Ordinary columns stay unmarked.
    assert!(place.member("name").unwrap().kind.opaque().is_none());
    assert_eq!(place.indexes[1].method.as_deref(), Some("gist"));
    assert!(place.indexes[2].raw.is_some());
}

#[test]
fn filtering_sorting_or_grouping_an_opaque_column_errors() {
    let (_, d) = analyze(
        r#"
        Place { id: Id, location: raw("geometry")? }
        shape Row from Place { id }
        query q() -> Row[] { list Place where (location = "x") unindexed(unsafe); }
        query s() -> Row[] { list Place order (location) unindexed(unsafe); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0271", "E0271"]);
}

#[test]
fn writing_an_opaque_column_errors() {
    let (_, d) = analyze(
        r#"
        Place { id: Id, location: raw("geometry")? }
        shape Row from Place { id }
        mutation m(v) -> Row { create Place { location = $v } }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0273"]);
}

#[test]
fn a_required_opaque_column_makes_create_impossible() {
    let (_, d) = analyze(
        r#"
        Place { id: Id, name: text, location: raw("geometry") }
        shape Row from Place { id }
        mutation m(n) -> Row { create Place { name = $n } }
        "#,
    );
    // E0273 replaces the ordinary "missing required field" (E0146) — there is no
    // value the caller could have supplied.
    assert_eq!(errors(&d), vec!["E0273"]);
}

#[test]
fn empty_raw_body_and_unknown_method_error() {
    let (_, d) = analyze(r#"Place { id: Id, a: raw("")?, @index raw("") @index a using nope }"#);
    let mut got = errors(&d);
    got.sort_unstable();
    assert_eq!(got, vec!["E0272", "E0274", "E0274"]);
}

#[test]
fn a_raw_map_naming_an_unknown_dialect_errors() {
    let (_, d) = analyze(r#"Place { id: Id, a: raw({ oracle: "clob" })? }"#);
    assert_eq!(errors(&d), vec!["E0270"]);
}

#[test]
fn check_target_reports_the_missing_dialect_and_unavailable_method() {
    let (schema, _) = analyze(PLACE);
    // The map covers all three targets and `gist` is a Postgres method.
    assert!(based_sema::check_target(&schema, "postgres").is_empty());
    let sqlite = based_sema::check_target(&schema, "sqlite");
    assert_eq!(
        codes(&sqlite),
        vec!["E0272"],
        "sqlite has no access methods"
    );

    let (partial, _) = analyze(r#"Place { id: Id, a: raw({ postgres: "tsvector" })? }"#);
    assert_eq!(
        codes(&based_sema::check_target(&partial, "mariadb")),
        vec!["E0270"]
    );
    assert!(based_sema::check_target(&partial, "postgres").is_empty());
}
