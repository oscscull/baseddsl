use super::*;

// ---------- nested to-one shape sub-objects (L1) ---------------------------

#[test]
fn nested_to_one_forward_projects_prefixed_columns() {
    // A to-one `placed_by { … }` nest projects the joined User's columns under a
    // `placed_by.`-prefixed alias the runtime reassembles into a sub-object. The join is
    // the same one a reach-rename would build (reused machinery).
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, email: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order { id: Id, deleted_at: timestamp?, placed_by: User, total: int, placed_at: timestamp }
        shape OrderCard from Order { total, placed_by { name, email } }
        query order_by_id(id) -> OrderCard;
        "#);
    // the local column stays flat, the nested columns carry the `placed_by.` prefix.
    assert!(ddl.contains("`order`.`total` AS `total`"), "\n{ddl}");
    assert!(
        ddl.contains("`j_placed_by`.`name` AS `placed_by.name`"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("`j_placed_by`.`email` AS `placed_by.email`"),
        "\n{ddl}"
    );
    // the relation join is materialized (required relation -> INNER JOIN).
    assert!(
        ddl.contains("JOIN `user` AS `j_placed_by` ON `j_placed_by`.`id` = `order`.`placed_by_id`"),
        "\n{ddl}"
    );
}

#[test]
fn optional_to_one_nest_projects_a_presence_probe() {
    // A LEFT-JOINed to-one nest (optional relation) can come back all-NULL when the
    // row is absent; the child's `id` is projected once more as `<field>.__present`
    // so the runtime can collapse the sub-object to JSON null. A required nest
    // inner-joins and needs no probe.
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, email: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order {
          id: Id
          deleted_at: timestamp?
          placed_by:  User
          courier:    User?
          total:      int
          placed_at:  timestamp
        }
        shape OrderCard from Order { total, placed_by { name }, courier { name, email } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains("`j_courier`.`id` AS `courier.__present`"),
        "\n{ddl}"
    );
    assert!(!ddl.contains("`placed_by.__present`"), "\n{ddl}");
}

#[test]
fn optional_to_one_nest_inside_a_json_array_null_collapses() {
    // Inside a to-many JSON aggregate the sub-object is built in SQL, so the absent
    // row collapses there: CASE WHEN the child's id IS NULL THEN NULL.
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order {
          id: Id
          deleted_at: timestamp?
          total:      int
          placed_at:  timestamp
          items:      OrderItem[] (OrderItem.order)
        }
        @soft_delete(deleted_at)
        OrderItem { id: Id, deleted_at: timestamp?, order: Order, checker: User?, qty: int }
        shape OrderCard from Order { total, items { qty, checker { name } } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains("CASE WHEN `j_checker`.`id` IS NULL THEN NULL ELSE"),
        "\n{ddl}"
    );
}

#[test]
fn nested_to_one_recurses_and_reaches_inside_nest() {
    // Nested-within-nested (`placed_by { org { name } }`) chains joins and deepens the
    // alias prefix (`placed_by.org.name`); a `=`-reach inside a nest resolves from the
    // nested model's alias.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        @sort(id asc)
        User { id: Id, org: Org, name: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int }
        shape OrderCard from Order { total, placed_by { name, org { name }, org_name = org.name } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains("`j_placed_by`.`name` AS `placed_by.name`"),
        "\n{ddl}"
    );
    // the doubly-nested column carries the full prefix chain.
    assert!(
        ddl.contains("`j_placed_by_org`.`name` AS `placed_by.org.name`"),
        "\n{ddl}"
    );
    // a `=`-reach inside the nest is prefixed by the nest it sits in.
    assert!(
        ddl.contains("`j_placed_by_org`.`name` AS `placed_by.org_name`"),
        "\n{ddl}"
    );
    // both joins exist, the second keyed off the first's alias.
    assert!(
        ddl.contains("JOIN `user` AS `j_placed_by` ON `j_placed_by`.`id` = `order`.`placed_by_id`"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains(
            "JOIN `org` AS `j_placed_by_org` ON `j_placed_by_org`.`id` = `j_placed_by`.`org_id`"
        ),
        "\n{ddl}"
    );
}

#[test]
fn nest_ref_lowers_identically_to_inline_nest() {
    // A named-shape nest (`placed_by -> UserRef`) is a pure body expansion: the
    // emitted SELECT is byte-identical to the same fields nested inline — for a
    // to-one edge and for a to-many edge (correlated JSON subquery) alike.
    let common = r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, email: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order { id: Id, deleted_at: timestamp?, placed_by: User, total: int, placed_at: timestamp,
                items: OrderItem[] }
        OrderItem { id: Id, order: Order, sku: text, qty: int }
    "#;
    let inline = gen(&format!(
        r#"{common}
        shape OrderDetail from Order {{ total, placed_by {{ name, email }}, items {{ sku, qty }} }}
        query order_detail(id) -> OrderDetail;
        "#
    ));
    let named = gen(&format!(
        r#"{common}
        shape UserRef from User {{ name, email }}
        shape ItemRow from OrderItem {{ sku, qty }}
        shape OrderDetail from Order {{ total, placed_by -> UserRef, items -> ItemRow }}
        query order_detail(id) -> OrderDetail;
        "#
    ));
    let section = |s: &str| query_section(s, "order_detail").to_string();
    assert_eq!(section(&inline), section(&named), "\n{named}");
}

#[test]
fn nest_ref_recurses_through_named_shapes() {
    // A referenced shape may itself reference: `placed_by -> UserRef` where UserRef
    // nests `org -> OrgRef` chains the joins and prefixes like inline nesting.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        @sort(id asc)
        User { id: Id, org: Org, name: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int }
        shape OrgRef from Org { name }
        shape UserRef from User { name, org -> OrgRef }
        shape OrderDetail from Order { total, placed_by -> UserRef }
        query order_detail(id) -> OrderDetail;
        "#);
    assert!(
        ddl.contains("`j_placed_by_org`.`name` AS `placed_by.org.name`"),
        "\n{ddl}"
    );
}
