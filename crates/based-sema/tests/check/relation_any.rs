use super::*;

const CARDS: &str = r#"
    Set { id: Id, code: text, cards: Card[] }
    Card { id: Id, set: Set, name: text, printings: Printing[] }
    Printing { id: Id, card: Card, digital: bool }
    shape CardRow from Card { name }
    shape SetRow from Set { code }
"#;

fn errs_with(query: &str) -> Vec<&'static str> {
    let src = format!("{CARDS}{query}");
    let (_, diags) = analyze(&src);
    diags
        .iter()
        .filter(|d| d.severity == Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect()
}

#[test]
fn any_over_a_to_many_relation_checks_clean() {
    let e = errs_with(
        "query paper() -> CardRow[] { list Card where (printings.any(digital = false)); }",
    );
    assert!(e.is_empty(), "{e:?}");
    let e = errs_with("query unused() -> CardRow[] { list Card where (not printings.any()); }");
    assert!(e.is_empty(), "{e:?}");
}

#[test]
fn any_through_a_to_one_hop_and_nested_any_check_clean() {
    let e = errs_with(
        "query siblings() -> CardRow[] { list Card where (set.cards.any(name = \"x\")); }",
    );
    assert!(e.is_empty(), "{e:?}");
    let e = errs_with(
        "query paper_sets() -> SetRow[] { list Set where (cards.any(printings.any(digital = false))); }",
    );
    assert!(e.is_empty(), "{e:?}");
}

#[test]
fn inner_predicate_resolves_against_the_related_model() {
    let e =
        errs_with("query bad() -> CardRow[] { list Card where (printings.any(name = \"x\")); }");
    assert_eq!(e, vec!["E0111"]);
}

#[test]
fn any_on_a_column_or_to_one_relation_is_an_error() {
    let e = errs_with("query bad() -> CardRow[] { list Card where (name.any()); }");
    assert_eq!(e, vec!["E0351"]);
    let e = errs_with("query bad() -> CardRow[] { list Card where (set.any()); }");
    assert_eq!(e, vec!["E0351"]);
}

#[test]
fn any_reached_through_a_to_many_hop_is_an_error() {
    let e = errs_with(
        "query bad() -> SetRow[] { list Set where (cards.printings.any(digital = false)); }",
    );
    assert_eq!(e, vec!["E0351"]);
}
