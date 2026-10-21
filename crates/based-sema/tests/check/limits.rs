use super::*;

// ---------- `limit (N | $p[, max N])` --------------------------------------

#[test]
fn param_and_literal_limits_are_clean() {
    let (_, diags) = analyze(
        r#"
        @sort(id asc)
        Run { id: Id, schedule_id: text, started_at: timestamp }
        query recent(limit: int) -> Run[] { list Run limit ($limit); }
        query capped(n: int) -> Run[] { list Run order (started_at desc) limit ($n, max 500); }
        query top() -> Run[] { list Run limit (3); }
        query inline_top() -> Run[] limit (3);
        "#,
    );
    assert!(errors(&diags).is_empty(), "{:?}", codes(&diags));
}

#[test]
fn limit_on_aggregate_query_is_clean() {
    let (_, diags) = analyze(
        r#"
        City { id: Id, region: text }
        shape RegionCount from City { region, n = count() }
        query top(n: int) -> RegionCount[] {
          list City group by (region) order (n desc) limit ($n);
        }
        "#,
    );
    assert!(errors(&diags).is_empty(), "{:?}", codes(&diags));
}

#[test]
fn limit_with_page_is_e0358() {
    let (_, diags) = analyze(
        r#"
        Item { id: Id, name: text }
        query items(n: int) -> Item[] { list Item page (20) limit ($n); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0358"), "{:?}", codes(&diags));
}

#[test]
fn limit_on_get_is_e0358() {
    let (_, diags) = analyze(
        r#"
        Item { id: Id, name: text }
        query item(id: Id) -> Item { get Item where (id = $id) limit (1); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0358"), "{:?}", codes(&diags));
}

#[test]
fn param_limit_in_inline_body_is_e0358() {
    let (_, diags) = analyze(
        r#"
        @sort(id asc)
        Item { id: Id, name: text }
        query items(n: int) -> Item[] limit ($n);
        "#,
    );
    assert!(errors(&diags).contains(&"E0358"), "{:?}", codes(&diags));
}

#[test]
fn zero_literal_limit_is_e0359() {
    let (_, diags) = analyze(
        r#"
        @sort(id asc)
        Item { id: Id, name: text }
        query items() -> Item[] { list Item limit (0); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0359"), "{:?}", codes(&diags));
}

#[test]
fn max_on_literal_limit_is_e0359() {
    let (_, diags) = analyze(
        r#"
        @sort(id asc)
        Item { id: Id, name: text }
        query items() -> Item[] { list Item limit (5, max 10); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0359"), "{:?}", codes(&diags));
}

#[test]
fn non_int_limit_param_is_e0359() {
    for sig in ["n: text", "n: int?", "n"] {
        let (_, diags) = analyze(&format!(
            r#"
            @sort(id asc)
            Item {{ id: Id, name: text }}
            query items({sig}) -> Item[] {{ list Item limit ($n); }}
            "#
        ));
        assert!(
            errors(&diags).contains(&"E0359"),
            "{sig}: {:?}",
            codes(&diags)
        );
    }
}

#[test]
fn unknown_limit_param_is_e0113() {
    let (_, diags) = analyze(
        r#"
        @sort(id asc)
        Item { id: Id, name: text }
        query items() -> Item[] { list Item limit ($n); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0113"), "{:?}", codes(&diags));
}
