use super::*;

#[test]
fn enum_where_variant_lowers_to_a_string_literal() {
    let src = r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query paid() -> OrderRow[] { list Order where (status = paid) order (total); }
    "#;
    let sql = gen(src);
    assert!(sql.contains("`order`.`status` = 'paid'"), "\n{sql}");
}

#[test]
fn string_enum_variant_lowers_to_its_wire_value() {
    let src = r#"
        enum Status { pending, paid = "PAID" }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query paid() -> OrderRow[] { list Order where (status = paid) order (total); }
    "#;
    let sql = gen(src);
    assert!(sql.contains("`order`.`status` = 'PAID'"), "\n{sql}");
}

#[test]
fn in_value_list_lowers_variants_params_and_literals_all_dialects() {
    let src = r#"
        enum Status { pending, paid = "PAID", shipped }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        query active(extra: Status) -> OrderRow[] {
          list Order where (status in (pending, paid, $extra) and total in (1, 2)) order (total);
        }
    "#;
    // Variants lower to wire values, the `$param` element to its own placeholder.
    let maria = gen(src);
    assert!(
        maria.contains("`order`.`status` IN ('pending', 'PAID', :extra)"),
        "\n{maria}"
    );
    assert!(maria.contains("`order`.`total` IN (1, 2)"), "\n{maria}");

    let pg = gen_pg(src);
    assert!(
        pg.contains(r#""order"."status" IN ('pending', 'PAID', :extra)"#),
        "\n{pg}"
    );

    let lite = gen_for(src, Dialect::Sqlite);
    assert!(
        lite.contains("`order`.`status` IN ('pending', 'PAID', :extra)"),
        "\n{lite}"
    );
}

#[test]
fn int_enum_in_value_list_lowers_to_integers() {
    let src = r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, priority: Priority, title: text }
        shape TicketRow from Ticket { priority, title }
        query hot() -> TicketRow[] { list Ticket where (priority in (medium, high)) order (title); }
    "#;
    let sql = gen(src);
    assert!(sql.contains("`ticket`.`priority` IN (1, 2)"), "\n{sql}");
}

#[test]
fn int_enum_variant_lowers_to_an_integer_literal() {
    let src = r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, priority: Priority, title: text }
        shape TicketRow from Ticket { priority, title }
        query urgent() -> TicketRow[] { list Ticket where (priority >= medium) order (title); }
    "#;
    let sql = gen(src);
    assert!(sql.contains("`ticket`.`priority` >= 1"), "\n{sql}");
}
