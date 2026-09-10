use super::*;

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
