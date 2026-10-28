use super::*;

// ---------- `~ all` / `~ any` pattern lists ----------------------------------

const CARD: &str = r#"
    Card { id: Id, name: text, body: text, cmc: int, @index name }
    shape C from Card { name }
"#;

fn with_card(src: &str) -> String {
    format!("{CARD}{src}")
}

#[test]
fn text_list_params_are_clean() {
    assert_clean(&with_card(
        r#"
        query q(terms: text[], banned?: text[]) -> C[] {
          list Card where (body ~ all $terms and not (body ~ any $banned)) order (name);
        }
        "#,
    ));
}

#[test]
fn text_list_binding_is_clean() {
    let (_, d) = analyze(&with_card("query bare(terms?: text[] ~ any body) -> C[];"));
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn text_list_through_a_named_filter_is_clean() {
    assert_clean(&with_card(
        r#"
        filter matches(include, exclude) = body ~ all $include and not (body ~ any $exclude);
        query q(include: text[], exclude: text[]) -> C[] {
          list Card where (matches($include, $exclude)) order (name);
        }
        "#,
    ));
}

#[test]
fn non_text_column_rejected() {
    let (_, d) = analyze(&with_card(
        r#"query q(terms: text[]) -> C[] { list Card where (cmc ~ all $terms) order (name); }"#,
    ));
    assert_eq!(errors(&d), vec!["E0150"]);
}

#[test]
fn literal_operand_rejected() {
    let (_, d) = analyze(&with_card(
        r#"query q() -> C[] { list Card where (body ~ any "%x%") order (name); }"#,
    ));
    assert_eq!(errors(&d), vec!["E0150"]);
}

#[test]
fn scalar_param_rejected() {
    let (_, d) = analyze(&with_card(
        r#"query q(term: text) -> C[] { list Card where (body ~ all $term) order (name); }"#,
    ));
    assert_eq!(errors(&d), vec!["E0152"]);
}

#[test]
fn untyped_param_rejected() {
    let (_, d) = analyze(&with_card(
        r#"query q(terms) -> C[] { list Card where (body ~ all $terms) order (name); }"#,
    ));
    assert_eq!(errors(&d), vec!["E0152"]);
}

#[test]
fn non_text_list_rejected() {
    let (_, d) = analyze(&with_card(
        r#"query q(terms: int[]) -> C[] { list Card where (body ~ any $terms) order (name); }"#,
    ));
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn scalar_param_through_a_named_filter_rejected() {
    let (_, d) = analyze(&with_card(
        r#"
        filter matches(include) = body ~ all $include;
        query q(include: text) -> C[] { list Card where (matches($include)) order (name); }
        "#,
    ));
    assert_eq!(errors(&d), vec!["E0152"]);
}

#[test]
fn scalar_binding_rejected() {
    let (_, d) = analyze(&with_card("query bare(body: text ~ all body) -> C[];"));
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn mutation_where_checked() {
    let (_, d) = analyze(&with_card(
        r#"mutation purge(terms: text) -> ok { delete Card where (body ~ any $terms); }"#,
    ));
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}
