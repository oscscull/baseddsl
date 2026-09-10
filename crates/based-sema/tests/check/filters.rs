use super::*;

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
