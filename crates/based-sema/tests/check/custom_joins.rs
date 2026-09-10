use super::*;

// ---------- custom `on:` joins ----------------------------------------------

#[test]
fn custom_join_resolves_clean() {
    // A legacy-key join: both sides are table-qualified columns that resolve
    // against the FK-holding model (`order`) and its target (`user`).
    assert_clean(
        r#"
        Order {
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = user.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
}

#[test]
fn custom_join_unknown_column_rejected() {
    // `user.nope` is not a column on the target model.
    let (_, d) = analyze(
        r#"
        Order {
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = user.nope)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0111"), "{:?}", codes(&d));
}

#[test]
fn custom_join_unknown_table_rejected() {
    // `customers` names no table in the two-table join scope.
    let (_, d) = analyze(
        r#"
        Order {
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = customers.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0125"), "{:?}", codes(&d));
}

#[test]
fn custom_join_unqualified_column_rejected() {
    // A join column must be `<table>.<column>`; a bare `user_ref` is malformed.
    let (_, d) = analyze(
        r#"
        Order {
          id: Id
          user_ref: int
          placed_by: User (on: user_ref = user.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0126"), "{:?}", codes(&d));
}

#[test]
fn custom_join_on_scalar_rejected() {
    // `on:` only makes sense on a to-one relation, not a scalar field.
    let (_, d) = analyze(
        r#"
        Order {
          id: Id
          user_ref: int (on: order.user_ref = user.legacy_id)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0126"), "{:?}", codes(&d));
}

#[test]
fn custom_join_param_rejected() {
    // A join is static structure — a request `$` param has no meaning here.
    let (_, d) = analyze(
        r#"
        Order {
          id: Id
          user_ref: int
          placed_by: User (on: order.user_ref = $x)
        }
        User { id: Id, name: text, legacy_id: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0126"), "{:?}", codes(&d));
}

#[test]
fn custom_join_self_ref_rejected() {
    // A self-ref custom join names both sides with the same table (`node.parent_ref =
    // node.id`), so codegen can't tell the near row from the joined row — rejected;
    // a self-relation uses the `<field>_id` convention instead.
    let (_, d) = analyze(
        r#"
        Node {
          id: Id
          parent_ref: int
          parent: Node (on: node.parent_ref = node.id)
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0127"), "{:?}", codes(&d));
}
