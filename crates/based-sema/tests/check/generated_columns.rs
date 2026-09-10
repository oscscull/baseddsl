use super::*;

// ---------- generated columns (E034x) --------------------------------------

#[test]
fn generated_column_infers_type_and_is_a_scalar() {
    let (schema, diags) =
        analyze(r#"Product { id: Id, price: decimal, discount: decimal, net = price - discount }"#);
    assert!(errors(&diags).is_empty(), "{:?}", codes(&diags));
    let m = schema.model("Product").unwrap();
    let net = m.member("net").unwrap();
    // A generated column resolves to a real scalar carrying its expression; the type is
    // inferred from the arithmetic (decimal - decimal -> decimal).
    match &net.kind {
        MemberKind::Scalar { ty, generated, .. } => {
            assert!(generated.is_some(), "net should be generated");
            assert!(
                matches!(ty, based_ast::Primitive::Decimal { .. }),
                "inferred type: {ty:?}"
            );
        }
        other => panic!("net is not a scalar: {other:?}"),
    }
}

#[test]
fn generated_column_reach_is_e0340() {
    let (_, d) = analyze(
        r#"
        Owner { id: Id, age: int }
        Cat { id: Id, owner: Owner, bad = owner.age + 1 }
        "#,
    );
    assert!(errors(&d).contains(&"E0340"), "{:?}", codes(&d));
}

#[test]
fn generated_column_unknown_column_is_e0341() {
    let (_, d) = analyze(r#"Product { id: Id, price: decimal, bad = nope * 2 }"#);
    assert!(errors(&d).contains(&"E0341"), "{:?}", codes(&d));
}

#[test]
fn generated_column_non_numeric_arith_is_e0343() {
    let (_, d) = analyze(r#"Product { id: Id, name: text, bad = name * 2 }"#);
    assert!(errors(&d).contains(&"E0343"), "{:?}", codes(&d));
}

#[test]
fn generated_column_non_text_concat_is_e0344() {
    let (_, d) = analyze(r#"Product { id: Id, qty: int, name: text, bad = qty || name }"#);
    assert!(errors(&d).contains(&"E0344"), "{:?}", codes(&d));
}

#[test]
fn generated_column_referencing_another_generated_is_e0346() {
    let (_, d) = analyze(
        r#"Product { id: Id, price: decimal, discount: decimal, net = price - discount, taxed = net * 2 }"#,
    );
    assert!(errors(&d).contains(&"E0346"), "{:?}", codes(&d));
}

#[test]
fn assigning_a_generated_column_is_e0347() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, price: decimal, discount: decimal, net = price - discount }
        shape Card from Product { id net }
        mutation make(p: decimal) -> Card { create Product { price = $p, discount = $p, net = 1 }; }
        "#,
    );
    assert!(errors(&d).contains(&"E0347"), "{:?}", codes(&d));
}

#[test]
fn generated_column_not_required_on_create() {
    // A generated column is NOT NULL with no default, yet a create needn't supply it.
    let (_, d) = analyze(
        r#"
        Product { id: Id, price: decimal, discount: decimal, net = price - discount }
        shape Card from Product { id net }
        mutation make(p: decimal) -> Card { create Product { price = $p, discount = $p }; }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}
