use super::*;

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

const SPREAD_SCHEMA: &str = r#"
    Org { id: Id, name: text }
    scope Tenant (org: Org = $ctx.org)
    @scope Tenant
    Inventory { id: Id, org: Org, sku: text, qty: int, price: int, @index(org, sku) unique }
    shape InvIn from Inventory { sku, qty, price }
"#;

#[test]
fn bulk_upsert_spread_incoming_is_ok() {
    let (_, d) = analyze(&format!(
        "{SPREAD_SCHEMA}
        mutation restock(rows: InvIn[]) -> ok scoped Tenant {{
          create Inventory[] from $rows on conflict (org, sku) update {{ ...incoming }};
        }}"
    ));
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn spread_on_inline_create_is_e0334() {
    let (_, d) = analyze(
        r#"
        Page { id: Id, path: text (unique), hits: int }
        shape PageRow from Page { path, hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { ...incoming };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0334"), "{:?}", codes(&d));
}

#[test]
fn spread_of_non_incoming_source_is_e0334() {
    let (_, d) = analyze(&format!(
        "{SPREAD_SCHEMA}
        mutation restock(rows: InvIn[]) -> ok scoped Tenant {{
          create Inventory[] from $rows on conflict (org, sku) update {{ ...nope }};
        }}"
    ));
    assert!(errors(&d).contains(&"E0334"), "{:?}", codes(&d));
}
