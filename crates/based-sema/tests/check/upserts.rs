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
fn upsert_composite_key_target_is_clean() {
    assert_clean(
        r#"
        @key(source, external_id)
        SyncState { source: text, external_id: text, snapshot: text }
        mutation set_sync_snapshot(source: text, external_id: text, snapshot: text) -> ok {
          create SyncState { source = $source, external_id = $external_id, snapshot = $snapshot }
            on conflict (external_id, source) update { snapshot = $snapshot };
        }
        "#,
    );
}

#[test]
fn upsert_partial_composite_key_target_is_e0250() {
    let (_, d) = analyze(
        r#"
        @key(source, external_id)
        SyncState { source: text, external_id: text, snapshot: text }
        mutation set_sync_snapshot(source: text, external_id: text, snapshot: text) -> ok {
          create SyncState { source = $source, external_id = $external_id, snapshot = $snapshot }
            on conflict (source) update { snapshot = $snapshot };
        }
        "#,
    );
    assert_eq!(errors(&d), ["E0250"]);
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

// ---------- `incoming.<relation>` in a bulk upsert ---------------------------

const INCOMING_REL_SCHEMA: &str = r#"
    Token { id: Id, name: text }
    Artist { id: Id, name: text }
    TokenPrinting {
      id: serial
      scryfall_id: text (unique)
      token: Token
      artist: Artist
      data: json
      hits: int
    }
    shape TokenPrintingIn from TokenPrinting { scryfall_id token { id } artist { id } data hits }
"#;

fn incoming_rel(update: &str) -> Vec<String> {
    let (_, d) = analyze(&format!(
        "{INCOMING_REL_SCHEMA}
        mutation import(rows: TokenPrintingIn[]) -> ok {{
          create TokenPrinting[] from $rows on conflict (scryfall_id) update {{ {update} }};
        }}"
    ));
    errors(&d).into_iter().map(str::to_string).collect()
}

#[test]
fn bulk_upsert_sets_relation_from_incoming() {
    let errs = incoming_rel("token = incoming.token, data = incoming.data");
    assert!(errs.is_empty(), "{errs:?}");
}

#[test]
fn incoming_relation_into_a_scalar_is_e0333() {
    assert_eq!(incoming_rel("data = incoming.token"), ["E0333"]);
}

#[test]
fn incoming_relation_into_another_model_is_e0333() {
    assert_eq!(incoming_rel("artist = incoming.token"), ["E0333"]);
}

#[test]
fn incoming_relation_in_arithmetic_is_e0333() {
    assert_eq!(incoming_rel("hits = hits + incoming.token"), ["E0333"]);
}

const INCOMING_COMPOSITE_SCHEMA: &str = r#"
    @key(code, term)
    Course { code: text, term: text, title: text }
    Seat { id: Id, tag: text (unique), label: text, course: Course }
    shape SeatIn from Seat { tag label course { code term } }
"#;

fn incoming_composite(update: &str) -> Vec<String> {
    let (_, d) = analyze(&format!(
        "{INCOMING_COMPOSITE_SCHEMA}
        mutation import(rows: SeatIn[]) -> ok {{
          create Seat[] from $rows on conflict (tag) update {{ {update} }};
        }}"
    ));
    errors(&d).into_iter().map(str::to_string).collect()
}

#[test]
fn bulk_upsert_sets_composite_relation_from_incoming() {
    let errs = incoming_composite("course = incoming.course");
    assert!(errs.is_empty(), "{errs:?}");
}

#[test]
fn incoming_scalar_into_a_composite_relation_is_e0333() {
    assert_eq!(incoming_composite("course = incoming.label"), ["E0333"]);
}
