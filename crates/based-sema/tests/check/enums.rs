use super::*;

// ---------- enums ----------------------------------------------------------

#[test]
fn enum_typed_field_is_a_scalar_not_a_relation() {
    let (schema, d) = analyze(
        r#"
        enum Status { pending, paid, shipped }
        Order { id: Id, status: Status, total: int }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let order = schema.model("Order").expect("Order model");
    match &order.member("status").expect("status member").kind {
        MemberKind::Scalar { enum_name, .. } => {
            assert_eq!(enum_name.as_deref(), Some("Status"));
        }
        other => panic!("status should be a scalar enum column, got {other:?}"),
    }
    assert!(schema
        .enum_("Status")
        .expect("enum resolved")
        .has_variant("paid"));
}

#[test]
fn enum_default_must_be_a_member() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status (default shipped), total: int }
        "#,
    );
    assert!(errors(&d).contains(&"E0155"), "{:?}", codes(&d));
}

#[test]
fn enum_default_member_is_clean() {
    assert_clean(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status (default pending), total: int }
        "#,
    );
}

#[test]
fn bare_default_on_non_enum_column_is_e0155() {
    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int (default whoops) }
        "#,
    );
    assert!(errors(&d).contains(&"E0155"), "{:?}", codes(&d));
}

#[test]
fn where_on_non_member_variant_is_e0154() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query paid_orders() -> OrderRow[] { list Order where (status = shipped); }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}

#[test]
fn where_on_member_variant_has_no_errors() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int, @index status }
        shape OrderRow from Order { status, total }
        query paid_orders() -> OrderRow[] { list Order where (status = paid) order (total); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn in_list_on_enum_column_checks_each_variant() {
    // Members + a `$param` element are clean; one borrowed variant is E0154.
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid, shipped }
        Order { id: Id, status: Status, total: int, @index status }
        shape OrderRow from Order { status, total }
        query open(extra: Status) -> OrderRow[] {
          list Order where (status in (pending, paid, $extra)) order (total);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));

    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        enum Priority { low = 1, high = 2 }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query open() -> OrderRow[] { list Order where (status in (pending, low)); }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}

#[test]
fn in_list_elements_are_family_checked() {
    // A text element in a numeric column's list is the same E0151 an `=` gives.
    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int, note: text }
        shape OrderRow from Order { total }
        query cheap() -> OrderRow[] { list Order where (total in (1, 2, "three")); }
        "#,
    );
    assert!(errors(&d).contains(&"E0151"), "{:?}", codes(&d));

    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int, note: text, @index total }
        shape OrderRow from Order { total }
        query cheap(x: int) -> OrderRow[] { list Order where (total in (1, 2, $x)); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn in_list_with_unknown_param_is_reported() {
    let (_, d) = analyze(
        r#"
        Order { id: Id, total: int }
        shape OrderRow from Order { total }
        query cheap() -> OrderRow[] { list Order where (total in (1, $nope)); }
        "#,
    );
    assert!(errors(&d).contains(&"E0113"), "{:?}", codes(&d));
}

#[test]
fn create_with_non_member_variant_is_e0154() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        mutation place() -> OrderRow { create Order { status = shipped, total = 1 } }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}

#[test]
fn create_with_member_variant_has_no_errors() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        mutation place() -> OrderRow { create Order { status = paid, total = 1 } }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn enum_name_colliding_with_a_model_is_e0106() {
    let (_, d) = analyze(
        r#"
        Status { id: Id, name: text }
        enum Status { pending, paid }
        "#,
    );
    assert!(errors(&d).contains(&"E0106"), "{:?}", codes(&d));
}

#[test]
fn string_enum_with_explicit_value_resolves() {
    let (schema, d) = analyze(
        r#"
        enum Status { pending, paid = "PAID", shipped }
        Order { id: Id, status: Status, total: int }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let en = schema.enum_("Status").expect("enum resolved");
    assert!(matches!(en.kind, based_sema::EnumKind::Str));
    assert_eq!(
        en.wire_of("paid"),
        Some(&based_sema::EnumValue::Str("PAID".into()))
    );
    assert_eq!(
        en.wire_of("pending"),
        Some(&based_sema::EnumValue::Str("pending".into()))
    );
    // The stored column is text for a string enum.
    match &schema
        .model("Order")
        .unwrap()
        .member("status")
        .unwrap()
        .kind
    {
        MemberKind::Scalar { ty, .. } => assert_eq!(*ty, based_ast::Primitive::Text),
        other => panic!("{other:?}"),
    }
}

#[test]
fn int_enum_resolves_and_is_an_integer_column() {
    let (schema, d) = analyze(
        r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, priority: Priority (default low), title: text }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let en = schema.enum_("Priority").expect("enum resolved");
    assert!(en.is_int());
    assert_eq!(en.wire_of("high"), Some(&based_sema::EnumValue::Int(2)));
    match &schema
        .model("Ticket")
        .unwrap()
        .member("priority")
        .unwrap()
        .kind
    {
        MemberKind::Scalar { ty, enum_name, .. } => {
            assert_eq!(*ty, based_ast::Primitive::Int);
            assert_eq!(enum_name.as_deref(), Some("Priority"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn mixed_int_and_bare_variants_is_e0156() {
    let (_, d) = analyze(
        r#"
        enum Bad { low = 0, medium, high = 2 }
        Ticket { id: Id, priority: Bad }
        "#,
    );
    assert!(errors(&d).contains(&"E0156"), "{:?}", codes(&d));
}

#[test]
fn mixed_int_and_string_variants_is_e0156() {
    let (_, d) = analyze(
        r#"
        enum Bad { low = 0, medium = "MID" }
        Ticket { id: Id, priority: Bad }
        "#,
    );
    assert!(errors(&d).contains(&"E0156"), "{:?}", codes(&d));
}

#[test]
fn duplicate_variant_value_is_e0157() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid = "pending" }
        Order { id: Id, status: Status }
        "#,
    );
    assert!(errors(&d).contains(&"E0157"), "{:?}", codes(&d));
}

#[test]
fn duplicate_int_variant_value_is_e0157() {
    let (_, d) = analyze(
        r#"
        enum Priority { low = 0, medium = 0 }
        Ticket { id: Id, priority: Priority }
        "#,
    );
    assert!(errors(&d).contains(&"E0157"), "{:?}", codes(&d));
}

#[test]
fn duplicate_variant_name_is_e0104() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, pending }
        Order { id: Id, status: Status }
        "#,
    );
    assert!(errors(&d).contains(&"E0104"), "{:?}", codes(&d));
}

#[test]
fn ordered_op_on_string_enum_is_e0158() {
    let (_, d) = analyze(
        r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query high() -> OrderRow[] { list Order where (status > pending); }
        "#,
    );
    assert!(errors(&d).contains(&"E0158"), "{:?}", codes(&d));
}

#[test]
fn ordered_op_on_int_enum_is_clean() {
    let (_, d) = analyze(
        r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, priority: Priority, title: text, @index priority }
        shape TicketRow from Ticket { priority, title }
        query urgent() -> TicketRow[] { list Ticket where (priority >= medium) order (title); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    assert!(!codes(&d).contains(&"E0158"), "{:?}", codes(&d));
}

#[test]
fn int_enum_variant_membership_still_checked_by_name() {
    let (_, d) = analyze(
        r#"
        enum Priority { low = 0, high = 1 }
        Ticket { id: Id, priority: Priority, title: text }
        shape TicketRow from Ticket { priority, title }
        query q() -> TicketRow[] { list Ticket where (priority = urgent); }
        "#,
    );
    assert!(errors(&d).contains(&"E0154"), "{:?}", codes(&d));
}
