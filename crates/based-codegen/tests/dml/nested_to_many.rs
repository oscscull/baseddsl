use super::*;

// ---------- nest-reached scoped children carry their `@scope` (H9) ----------

#[test]
fn nest_only_to_one_scoped_child_injects_scope_in_join_on() {
    // A scoped child reached *only* through a to-one nest (`raised_by { … }`, not via
    // any where/order/reach) still carries its `@scope` into the nest join's `ON`, so
    // a nested sub-object can't read a row across the scope boundary. `Contact` is
    // org-scoped; `Ticket` is not.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, raised_by { name } }
        query ticket_by_id(id) -> TicketCard scoped Tenant;
        "#);
    assert!(
        ddl.contains(
            "JOIN `contact` AS `j_raised_by` ON `j_raised_by`.`id` = `ticket`.`raised_by_id` AND `j_raised_by`.`org_id` = :ctx_org"
        ),
        "\n{ddl}"
    );
}

#[test]
fn nest_only_to_many_scoped_child_injects_scope_in_subquery_where() {
    // A scoped child reached *only* through a to-many nest (`items { … }`) carries its
    // `@scope` into the correlated subquery's `WHERE`, beside the correlation and the
    // tombstone. `LineItem` is org-scoped; `Order` is not.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @sort(id asc)
        Order { id: Id, total: int, items: LineItem[] }
        @scope Tenant
        @sort(id asc)
        LineItem { id: Id, order: Order, org: Org, sku: text }
        shape OrderCard from Order { total, items { sku } }
        query order_by_id(id) -> OrderCard scoped Tenant;
        "#);
    assert!(
        ddl.contains(
            "FROM `line_item` AS `s1_line_item` WHERE `s1_line_item`.`order_id` = `order`.`id` AND `s1_line_item`.`org_id` = :ctx_org) AS `items[]`"
        ),
        "\n{ddl}"
    );
}

#[test]
fn nested_to_many_aggregates_into_a_json_array_column() {
    // A to-many nest (`items { … }`, an inverse collection) lowers to a correlated
    // subquery aggregating the child rows into a JSON-array column aliased `items[]`
    // (the runtime parses the string into an array). The child's soft-delete tombstone
    // rides the subquery WHERE, and the subquery correlates on the child's back FK.
    let ddl = gen(r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] }
        @sort(id asc)
        @soft_delete(deleted_at)
        OrderItem { id: Id, order: Order, quantity: int, deleted_at: timestamp? }
        shape OrderCard from Order { total, items { quantity } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(ddl.contains("`order`.`total` AS `total`"), "\n{ddl}");
    // MariaDB JSON aggregation, ordered by the child model's `@sort` inside the
    // aggregate, coalesced to an empty array for a childless parent.
    assert!(
        ddl.contains("COALESCE(JSON_ARRAYAGG(JSON_OBJECT('quantity', `s1_order_item`.`quantity`) ORDER BY `s1_order_item`.`id` ASC), JSON_ARRAY())"),
        "\n{ddl}"
    );
    // correlated subquery over a distinctly-aliased child + the tombstone, aliased `items[]`.
    assert!(
        ddl.contains("FROM `order_item` AS `s1_order_item` WHERE `s1_order_item`.`order_id` = `order`.`id` AND `s1_order_item`.`deleted_at` IS NULL) AS `items[]`"),
        "\n{ddl}"
    );
    // it is a subquery in the SELECT list, not a join into the FROM.
    assert!(!ddl.contains("JOIN `order_item`"), "\n{ddl}");
}

#[test]
fn nested_self_referential_to_many_aliases_child_distinctly() {
    // The flagship self-ref case (`User.invited_users`): the subquery's child alias
    // (`s1_user`) must differ from the outer `user` row so the correlation is unambiguous.
    let ddl = gen(r#"
        @sort(id asc)
        User { id: Id, name: text, invited_by: User?, invited_users: User[] (User.invited_by) }
        shape UserCard from User { name, invited_users { name } }
        query user_by_id(id) -> UserCard;
        "#);
    assert!(
        ddl.contains("JSON_OBJECT('name', `s1_user`.`name`)"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("FROM `user` AS `s1_user` WHERE `s1_user`.`invited_by_id` = `user`.`id`) AS `invited_users[]`"),
        "\n{ddl}"
    );
}

#[test]
fn nested_to_many_relation_sort_overrides_child_model_sort() {
    // The traversal tier of the sort cascade: the edge's relation `@sort` (rank desc)
    // beats the child model's own `@sort` (id asc) inside the aggregate's ORDER BY.
    let ddl = gen(r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] @sort(rank desc) }
        @sort(id asc)
        OrderItem { id: Id, order: Order, rank: int, sku: text }
        shape OrderCard from Order { total, items { sku } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains(
            "JSON_OBJECT('sku', `s1_order_item`.`sku`) ORDER BY `s1_order_item`.`rank` DESC)"
        ),
        "\n{ddl}"
    );
}

#[test]
fn nested_to_many_without_any_sort_stays_unordered() {
    // No relation `@sort`, no child model `@sort` → no ORDER BY inside the aggregate;
    // the array stays an unordered set (as before).
    let ddl = gen(r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] }
        OrderItem { id: Id, order: Order, sku: text }
        shape OrderCard from Order { total, items { sku } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains(
            "COALESCE(JSON_ARRAYAGG(JSON_OBJECT('sku', `s1_order_item`.`sku`)), JSON_ARRAY())"
        ),
        "\n{ddl}"
    );
}
