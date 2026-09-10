use super::*;

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
