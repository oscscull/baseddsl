use super::*;

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
