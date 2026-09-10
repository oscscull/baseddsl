use super::*;

// ---------- @key(field) natural single-column primary key -----------------

#[test]
fn key_nominates_a_field_as_the_primary_key() {
    // `@key(sku)` makes an existing column the PK — no synthesized `id`, no E0261, and the
    // nominated field is unique (a `get` may key on it).
    let (schema, d) = analyze(
        r#"
        @key(sku)
        Product { sku: text  name: text }
        shape P from Product { sku, name }
        query product(sku) -> P;
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", errors(&d));
    let m = schema.model("Product").unwrap();
    assert_eq!(m.pk_field(), Some("sku"));
    assert_eq!(m.pk_column().as_deref(), Some("sku"));
    assert!(m.is_unique("sku"));
    // A natural key has no engine generation strategy (its value is app-supplied).
    assert_eq!(m.pk_strategy(), None);
    assert!(!m.pk_is_db_generated());
}

#[test]
fn key_names_an_unknown_field() {
    // `@key(x)` where `x` is not declared — E0275.
    let (_, d) = analyze(
        r#"
        @key(x)
        Product { sku: text  name: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0275"), "{:?}", errors(&d));
}

#[test]
fn key_field_must_be_a_required_scalar() {
    // An optional column can't be a primary key — E0276.
    let (_, d) = analyze(
        r#"
        @key(sku)
        Product { sku: text?  name: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0276"), "{:?}", errors(&d));
    // A relation can't be a single-column natural key either (composite FK-keys are PR6).
    assert_clean(
        r#"
        Org { id: Id  name: text }
        @key(org)
        Membership { org: Org  role: text }
        "#,
    );
}

#[test]
fn key_and_no_id_conflict() {
    // A model either nominates a key or declares itself keyless, never both — E0277.
    let (_, d) = analyze(
        r#"
        @key(sku)
        @no_id("legacy")
        Product { sku: text  name: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0277"), "{:?}", errors(&d));
}

#[test]
fn composite_key_is_supported() {
    // `@key(a, b)` is a composite primary key over two declared columns — clean, no `id`
    // synthesized, no E0261/E0278.
    assert_clean(
        r#"
        Order { id: Id  n: int }
        Product { id: Id  n: int }
        @key(order, product)
        Enrollment { order: Order  product: Product  at: timestamp }
        "#,
    );
}

#[test]
fn composite_key_rejects_empty_and_duplicate() {
    // `@key()` names no column — E0278.
    let (_, d) = analyze(
        r#"
        @key()
        Enrollment { order: text  product: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0278"), "{:?}", errors(&d));
    // `@key(a, a)` repeats a column — E0279.
    let (_, d) = analyze(
        r#"
        @key(order, order)
        Enrollment { order: text  product: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0279"), "{:?}", errors(&d));
}

#[test]
fn relation_to_a_key_model_is_clean() {
    // A forward relation into a `@key` model resolves against its nominated key (the model
    // is *not* keyless, so no E0265).
    assert_clean(
        r#"
        @key(sku)
        Product { sku: text  name: text }
        Order { id: Id  product: Product }
        "#,
    );
}

#[test]
fn manifest_id_default_resolves_bare_id() {
    use based_sema::{resolve_pk_default, PkStrategy};
    // `id: Id` follows the project default. Unresolved (no manifest pass) it reads as uuid.
    let (mut schema, _) = analyze("Org { id: Id  name: text }");
    assert_eq!(
        schema.model("Org").unwrap().pk_strategy(),
        Some(PkStrategy::Uuid)
    );
    // A `serial` project default rewrites `id: Id` to a serial PK.
    resolve_pk_default(&mut schema, PkStrategy::Serial);
    assert_eq!(
        schema.model("Org").unwrap().pk_strategy(),
        Some(PkStrategy::Serial)
    );
    // An explicit per-model type is never overridden by the default.
    let (mut schema, _) = analyze("Org { id: uuid  name: text }");
    resolve_pk_default(&mut schema, PkStrategy::Serial);
    assert_eq!(
        schema.model("Org").unwrap().pk_strategy(),
        Some(PkStrategy::Uuid)
    );
}

#[test]
fn bare_int_id_is_rejected() {
    // A DB-generated integer key must be spelled `serial` (its generation is visible),
    // not a bare `int` — E0266.
    let (_, d) = analyze("Counter { id: int  name: text }");
    assert_eq!(errors(&d), ["E0266"]);
    // A string natural key stays fine.
    let (_, d) = analyze("Country { id: text  name: text }");
    assert!(errors(&d).is_empty(), "{:?}", errors(&d));
}

#[test]
fn serial_and_ulid_are_pk_only_types() {
    // `serial`/`ulid` are primary-key generation strategies — not ordinary column types.
    let (_, d) = analyze("Widget { id: Id  seq: serial }");
    assert_eq!(errors(&d), ["E0267"]);
    let (_, d) = analyze("Widget { id: Id  code: ulid }");
    assert_eq!(errors(&d), ["E0267"]);
    // As the `id`, both are legal.
    let (_, d) = analyze("A { id: serial  name: text }\nB { id: ulid  name: text }");
    assert!(errors(&d).is_empty(), "{:?}", errors(&d));
}

#[test]
fn serial_is_legal_as_a_composite_key_part() {
    // A `serial` part inside a composite `@key(…)` is a DB-generated key column (OP2) —
    // legal, unlike a plain non-key `serial` (E0267).
    let src = r#"
        Device { id: Id  name: text }
        @key(device, seq)
        Reading { device: Device  seq: serial  value: int }
    "#;
    let (_, d) = analyze(src);
    assert!(errors(&d).is_empty(), "{:?}", errors(&d));
}

#[test]
fn single_column_serial_key_is_rejected() {
    // A `serial` is legal as the `id` or a composite key part — not a single-column
    // `@key(seq)` (use `id: serial`).
    let (_, d) = analyze("@key(seq)\nWidget { seq: serial  name: text }");
    assert_eq!(errors(&d), ["E0267"]);
}

#[test]
fn a_composite_key_allows_only_one_serial_part() {
    // A table has at most one DB-generated (auto-increment) column, so two `serial` key
    // parts is E0282.
    let src = r#"
        Device { id: Id }
        @key(device, a, b)
        Widget { device: Device  a: serial  b: serial  name: text }
    "#;
    let (_, d) = analyze(src);
    assert!(errors(&d).contains(&"E0282"), "{:?}", errors(&d));
}

#[test]
fn serial_id_can_be_reached_across_tx_steps() {
    // A bound create re-selects its written row, so a `$name.id` reference to a `serial`
    // (DB-generated) create resolves to the committed id — no E0268 (retired, D124). The
    // reference still validates the field is a member and the assign types agree.
    let src = r#"
        Org { id: serial  name: text }
        Note { id: Id  org: Org  body: text }
        shape NoteCard from Note { body }
        mutation setup(name, body) -> NoteCard {
          tx {
            create Org { name = $name } as o;
            create Note { org = $o.id, body = $body };
          }
        }
    "#;
    let analyzed = analyze(src);
    let errs = errors(&analyzed.1);
    assert!(
        errs.is_empty(),
        "serial `$o.id` should check clean: {errs:?}"
    );
    // The same shape with a uuid parent also binds fine.
    let ok = r#"
        Org { id: Id  name: text }
        Note { id: Id  org: Org  body: text }
        shape NoteCard from Note { body }
        mutation setup(name, body) -> NoteCard {
          tx {
            create Org { name = $name } as o;
            create Note { org = $o.id, body = $body };
          }
        }
    "#;
    assert!(
        errors(&analyze(ok).1).is_empty(),
        "{:?}",
        errors(&analyze(ok).1)
    );
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
