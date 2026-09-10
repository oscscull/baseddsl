use super::*;

#[test]
fn shape_spread_composition_expands_columns() {
    // `...UserBase` splices the base shape's columns; the SELECT projects the composed
    // set (base columns + the local `bio`). Mirrors the real front end: expand, then check.
    let sf = parse_file(
        r#"
        User { id: Id, name: text, email: text, bio: text }
        shape UserBase from User { id, name, email }
        shape UserCard from User { ...UserBase, bio }
        query get_user(id) -> UserCard;
        "#,
        FileId(0),
    )
    .unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let mut decls = sf.decls;
    let diags = based_sema::expand_spreads(&mut decls);
    assert!(diags.is_empty(), "spread errors: {diags:#?}");
    let (schema, _) = check(&decls);
    let ddl = sql::dml::dml(&schema, &decls, Dialect::MariaDb);
    for col in ["id", "name", "email", "bio"] {
        assert!(
            ddl.contains(&format!("`user`.`{col}` AS `{col}`")),
            "missing column {col}:\n{ddl}"
        );
    }
}

#[test]
fn bare_get_injects_soft_delete_and_maps_param() {
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp?, status: text, total: int }
        shape OrderCard from Order { status, total }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(ddl.contains("FROM `order`"), "\n{ddl}");
    assert!(ddl.contains("`order`.`status` AS `status`"), "\n{ddl}");
    // same-name param -> equality on the mapped column, ANDed with the tombstone.
    assert!(
        ddl.contains("WHERE `order`.`id` = :id AND `order`.`deleted_at` IS NULL"),
        "\n{ddl}"
    );
}

#[test]
fn schema_qualified_model_namespaces_from_and_join_not_column_refs() {
    // A `@schema`-qualified model reads `FROM schema.table`, but column references keep the
    // bare table name as the correlation (SQL's implicit alias) — so only the base object
    // carries the namespace. A join into another qualified model likewise qualifies its
    // JOIN target while aliasing normally.
    let ddl = gen(r#"
        @schema("core")
        Org { id: Id, name: text }
        @schema("analytics")
        Event { id: Id, org: Org, note: text }
        shape EventCard from Event { note, org { name } }
        query event_by_id(id) -> EventCard;
        "#);
    assert!(ddl.contains("FROM `analytics`.`event`"), "\n{ddl}");
    assert!(ddl.contains("JOIN `core`.`org` AS"), "\n{ddl}");
    // column refs use the bare table name, never the schema-qualified form.
    assert!(ddl.contains("`event`.`note`"), "\n{ddl}");
    assert!(!ddl.contains("`analytics.event`"), "\n{ddl}");
    assert!(ddl.contains("WHERE `event`.`id` = :id"), "\n{ddl}");
}

#[test]
fn relation_param_maps_to_fk_column() {
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        Org { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp?, org: Org, total: int }
        shape OrderCard from Order { total }
        query orders(org) -> OrderCard[];
        "#);
    // a relation same-name param compares the FK column, not a join.
    assert!(ddl.contains("WHERE `order`.`org_id` = :org"), "\n{ddl}");
}

#[test]
fn shape_reach_joins_and_injects_soft_delete_in_on() {
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order { id: Id, deleted_at: timestamp?, placed_by: User, placed_at: timestamp }
        shape OrderCard from Order { buyer = placed_by.name }
        query order_by_id(id) -> OrderCard;
        "#);
    // required relation -> INNER JOIN, aliased by path prefix, soft-delete in ON.
    assert!(
        ddl.contains("JOIN `user` AS `j_placed_by` ON `j_placed_by`.`id` = `order`.`placed_by_id` AND `j_placed_by`.`deleted_at` IS NULL"),
        "\n{ddl}"
    );
    assert!(ddl.contains("`j_placed_by`.`name` AS `buyer`"), "\n{ddl}");
}

#[test]
fn shape_reach_into_scoped_model_injects_scope_in_join_on() {
    // a query reaching a *scoped* model through a relation carries that model's
    // `@scope` into the join `ON` — same slot as soft-delete — so it can't read a row
    // across the scope boundary. Here `Contact` is org-scoped and reached via a shape.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query ticket_by_id(id) -> TicketCard scoped Tenant;
        "#);
    // The join into the scoped `contact` ANDs `contact.org_id = :ctx_org` into its ON.
    assert!(
        ddl.contains(
            "JOIN `contact` AS `j_raised_by` ON `j_raised_by`.`id` = `ticket`.`raised_by_id` AND `j_raised_by`.`org_id` = :ctx_org"
        ),
        "\n{ddl}"
    );
}

#[test]
fn where_reach_into_scoped_model_injects_scope_in_join_on() {
    // The same injection fires for a relation reached in a `where`, not just a shape.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject }
        query tickets_by_contact_name(name) -> TicketCard[] scoped Tenant {
          list Ticket where (raised_by.name = $name);
        }
        "#);
    assert!(
        ddl.contains(
            "JOIN `contact` AS `j_raised_by` ON `j_raised_by`.`id` = `ticket`.`raised_by_id` AND `j_raised_by`.`org_id` = :ctx_org"
        ),
        "\n{ddl}"
    );
}

#[test]
fn unscoped_query_drops_joined_scope_too() {
    // `unscoped`  opts out of *all* scope handling — the joined table's `@scope`
    //  included, not just the root's. The join `ON` carries no scope predicate.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query any_ticket(id) -> TicketCard unscoped("admin: cross-org ticket lookup");
        "#);
    assert!(
        ddl.contains(
            "JOIN `contact` AS `j_raised_by` ON `j_raised_by`.`id` = `ticket`.`raised_by_id`"
        ),
        "\n{ddl}"
    );
    assert!(
        !ddl.contains(":ctx_org"),
        "unscoped must inject no scope\n{ddl}"
    );
}

#[test]
fn optional_relation_is_left_join() {
    let ddl = gen(r#"
        User { id: Id, name: text }
        @sort(id asc)
        Order { id: Id, fulfilled_by: User?, total: int }
        shape OrderCard from Order { fulfiller = fulfilled_by.name }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains("LEFT JOIN `user` AS `j_fulfilled_by`"),
        "\n{ddl}"
    );
}

#[test]
fn edge_binding_and_colop_binding() {
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        @sort(created_at desc)
        Post { id: Id, deleted_at: timestamp?, author: User, created_at: timestamp }
        shape PostShape from Post { created_at }
        query posts(user -> author, since: timestamp > created_at) -> PostShape[];
        "#);
    // `user -> author` binds the relation FK; `since > created_at` uses the operator.
    assert!(ddl.contains("`post`.`author_id` = :user"), "\n{ddl}");
    assert!(ddl.contains("`post`.`created_at` > :since"), "\n{ddl}");
}
