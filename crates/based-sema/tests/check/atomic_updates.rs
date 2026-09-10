use super::*;

// ---------- atomic update expressions --------------------------------------

#[test]
fn atomic_update_numeric_expr_is_clean() {
    // A self-referential arithmetic SET over numeric columns + a param.
    assert_clean(
        r#"
        Product { id: Id, qty: int, price: decimal(10, 2) }
        shape P from Product { qty, price }
        mutation adjust(id: Id, delta: int) -> P {
          update Product where (id = $id) { qty = qty + $delta };
        }
        mutation markup(id: Id, factor: decimal(10, 2)) -> P {
          update Product where (id = $id) { price = price * $factor };
        }
        "#,
    );
}

#[test]
fn atomic_update_precedence_and_parens_clean() {
    assert_clean(
        r#"
        Product { id: Id, qty: int, price: decimal(10, 2) }
        shape P from Product { qty, price }
        mutation recompute(id: Id, base: int, n: int) -> P {
          update Product where (id = $id) { qty = (qty + $base) * $n - 1 };
        }
        "#,
    );
}

#[test]
fn atomic_update_expr_in_create_rejected() {
    // A `create` has no existing row to self-reference — arithmetic is update-only.
    let (_, d) = analyze(
        r#"
        Product { id: Id, qty: int }
        shape P from Product { qty }
        mutation make(n: int) -> P { create Product { qty = qty + $n }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0230"]);
}

#[test]
fn atomic_update_nonnumeric_column_operand_rejected() {
    // A text column can't be an arithmetic operand.
    let (_, d) = analyze(
        r#"
        Product { id: Id, qty: int, name: text }
        shape P from Product { qty }
        mutation bump(id: Id, n: int) -> P {
          update Product where (id = $id) { qty = name + $n };
        }
        "#,
    );
    assert_eq!(errors(&d), ["E0231"]);
}

#[test]
fn atomic_update_nonnumeric_target_rejected() {
    // Assigning a numeric arithmetic result to a text column is the ordinary E0153.
    let (_, d) = analyze(
        r#"
        Product { id: Id, qty: int, name: text }
        shape P from Product { name }
        mutation bump(id: Id, n: int) -> P {
          update Product where (id = $id) { name = qty + $n };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0153"), "{:?}", codes(&d));
}
