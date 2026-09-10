use super::*;

// ---------- optional filter params (`name?`) -------------------------------

const OPT_MODEL: &str = r#"
    Product {
      id: Id
      name: text
      status: text?
      created_at: timestamp
      @index(status)
      @index(created_at)
    }
    shape Card from Product { name }
"#;

#[test]
fn optional_filter_param_checks_clean() {
    let src = format!("{OPT_MODEL}\nquery search(status?) -> Card[];");
    let (_, d) = analyze(&src);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn optional_filter_on_get_is_e0335() {
    let src = format!("{OPT_MODEL}\nquery one(status?) -> Card;");
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0335"), "{:?}", codes(&d));
}

#[test]
fn optional_filter_with_default_is_e0336() {
    let src = format!("{OPT_MODEL}\nquery search(status? = \"x\") -> Card[];");
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0336"), "{:?}", codes(&d));
}

#[test]
fn optional_filter_with_operator_binding_is_clean() {
    // `?` works with any operator now (E0337 retired) — the guard drops the whole predicate
    // when the arg is absent, regardless of operator.
    let src = format!("{OPT_MODEL}\nquery search(since?: timestamp > created_at) -> Card[];");
    let (_, d) = analyze(&src);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn optional_filter_with_like_and_range_is_clean() {
    let src = format!(
        "{OPT_MODEL}\nquery search(pat?: text ~ status, after?: timestamp > created_at) -> Card[];"
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn optional_filter_in_block_where_is_clean() {
    // A `?` param referenced in a block `where` is present-guarded — allowed now (was E0338).
    let src = format!(
        "{OPT_MODEL}\nquery search(status?) -> Card[] {{ list Product where (status = $status); }}"
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn optional_filter_or_composed_in_block_is_clean() {
    // The point of the feature: an optional `?` param inside an or-composed block filter checks
    // clean; each leaf present-guards independently, so an absent arg widens its `or` branch.
    let src = format!(
        "{OPT_MODEL}\nquery search(q?) -> Card[] {{ list Product where (name ~ $q or status ~ $q); }}"
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn optional_filter_on_raw_query_is_e0338() {
    // A raw body is verbatim SQL — a `?` can't be present-guarded, so it's rejected.
    let src = format!(
        "{OPT_MODEL}\nquery search(status?: text) -> Card[] {{ raw`SELECT name FROM product WHERE status = ${{status}}`; }}"
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0338"), "{:?}", codes(&d));
}

#[test]
fn optional_filter_on_mutation_param_is_e0338() {
    let src = format!(
        "{OPT_MODEL}\nmutation wipe(status?) -> ok {{ hard delete Product where (status = $status); }}"
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0338"), "{:?}", codes(&d));
}
