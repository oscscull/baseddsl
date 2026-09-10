use super::*;

// ---------- decimal / float (D83) ------------------------------------------

#[test]
fn decimal_and_float_columns_resolve() {
    let (schema, d) = analyze(
        r#"
        Ledger {
          id: Id
          price: decimal(12, 2)
          bare:  decimal
          score: float
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let m = schema.model("Ledger").unwrap();
    for f in ["price", "bare", "score"] {
        assert!(matches!(
            m.member(f).unwrap().kind,
            MemberKind::Scalar { .. }
        ));
    }
}

#[test]
fn bad_decimal_precision_scale_errors() {
    // scale > precision
    let (_, d) = analyze("Ledger { id: Id, price: decimal(2, 5) }");
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
    // precision over the 38 cap
    let (_, d) = analyze("Ledger { id: Id, price: decimal(40, 2) }");
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
    // scale zero (need 1 <= scale)
    let (_, d) = analyze("Ledger { id: Id, price: decimal(10, 0) }");
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
}

#[test]
fn numeric_literal_binds_to_decimal_and_float_columns() {
    let (_, d) = analyze(
        r#"
        Ledger { id: Id, price: decimal(12, 2), score: float, name: text }
        shape LedgerRow from Ledger { price, score }
        query dear() -> LedgerRow[] { list Ledger where (price > 9.99) unindexed(unsafe) order (name); }
        query fast() -> LedgerRow[] { list Ledger where (score >= 0.5) unindexed(unsafe) order (name); }
        "#,
    );
    // A numeric literal is compatible with a decimal/float column (no E0150/E0151).
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn ordered_compare_on_decimal_allowed() {
    let (_, d) = analyze(
        r#"
        Ledger { id: Id, price: decimal(12, 2), name: text }
        shape LedgerRow from Ledger { price }
        query pricey(min: decimal(12, 2) >= price) -> LedgerRow[] { list Ledger unindexed(unsafe) order (name); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn decimal_default_must_be_a_decimal_literal() {
    let (_, d) = analyze(r#"Ledger { price: decimal(12, 2) (default "x") }"#);
    assert!(errors(&d).contains(&"E0159"), "{:?}", codes(&d));
    // an integer or a fractional literal is fine
    assert_clean(
        "Ledger { id: Id, a: decimal(12, 2) (default 5), b: decimal(12, 2) (default 9.99) }",
    );
}
