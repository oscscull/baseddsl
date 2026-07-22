//! DML (query -> SELECT) codegen tests. Parse + check a whole-schema snippet, then
//! assert on the generated SELECT text. The headline assertions are the soft-delete
//! injection (root `WHERE` + every join `ON`) and the sort/pagination cascade.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

fn gen(src: &str) -> String {
    gen_for(src, Dialect::MariaDb)
}

fn gen_pg(src: &str) -> String {
    gen_for(src, Dialect::Postgres)
}

fn gen_for(src: &str, dialect: Dialect) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    // These snippets exercise SELECT lowering, not index completeness — a query that
    // scans an unindexed column (`E0260`) still lowers to correct SQL, and the index
    // requirement is covered authoritatively in based-sema's tests + conformance.
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    sql::dml::dml(&schema, &sf.decls, dialect)
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

#[test]
fn block_query_where_order_page_and_bare_bool() {
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        Product {
          id: Id
          deleted_at: timestamp?
          created_at: timestamp
          org: Org
          active: bool (default true)
        }
        Org { id: Id, name: text }
        shape ProductCard from Product { active }
        query active_products(org: Id) -> ProductCard[] {
          list Product
            where (org = $org and active)
            order (created_at desc)
            page (20);
        }
        "#);
    // bare bool column -> `= TRUE`; `$org` -> `:org`; tombstone ANDed on.
    assert!(
        ddl.contains("(`product`.`org_id` = :org AND `product`.`active` = TRUE)"),
        "\n{ddl}"
    );
    assert!(ddl.contains("`product`.`deleted_at` IS NULL"), "\n{ddl}");
    // keyset pagination appends the unique `id` tiebreaker (shown, not written).
    assert!(
        ddl.contains("ORDER BY `product`.`created_at` DESC, `product`.`id` ASC"),
        "\n{ddl}"
    );
    assert!(ddl.contains("LIMIT 20"), "\n{ddl}");
    assert!(
        !ddl.contains("OFFSET"),
        "keyset must not emit OFFSET:\n{ddl}"
    );
    // keyset cursor comparison: lexicographic over the sort keys, guarded by
    // `:keyset_active` (a no-op on page 1). `created_at DESC` compares `<`; the `id ASC`
    // tiebreaker compares `>` behind the `created_at =` equality prefix.
    assert!(ddl.contains(":keyset_active = 0 OR"), "\n{ddl}");
    assert!(
        ddl.contains("`product`.`created_at` < :keyset_0"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("`product`.`created_at` = :keyset_0 AND `product`.`id` > :keyset_1"),
        "\n{ddl}"
    );
    // hidden cursor-basis columns the runtime reads to mint the next cursor.
    assert!(
        ddl.contains("`product`.`created_at` AS `__keyset_0`"),
        "\n{ddl}"
    );
    assert!(ddl.contains("`product`.`id` AS `__keyset_1`"), "\n{ddl}");
}

#[test]
fn scope_predicate_is_injected() {
    let ddl = gen(r#"
        scope Tenant (org: Org = $ctx.org)
        @soft_delete(deleted_at)
        @scope Tenant
        @sort(id asc)
        Order { id: Id, deleted_at: timestamp?, org: Org, total: int }
        Org { id: Id, name: text }
        shape OrderCard from Order { total }
        query orders() -> OrderCard[] scoped Tenant;
        "#);
    // @scope rides the same injection path; `$ctx.org` -> `:ctx_org`.
    assert!(ddl.contains("`order`.`org_id` = :ctx_org"), "\n{ddl}");
    assert!(ddl.contains("`order`.`deleted_at` IS NULL"), "\n{ddl}");
}

#[test]
fn unscoped_query_omits_the_scope_predicate() {
    // `unscoped(...)`  is the cross-scope escape hatch: no `@scope` injection for
    // this query. Soft-delete still rides — it's a separate guarantee.
    let ddl = gen(r#"
        scope Tenant (org: Org = $ctx.org)
        @soft_delete(deleted_at)
        @scope Tenant
        @sort(id asc)
        Order { id: Id, deleted_at: timestamp?, org: Org, total: int }
        Org { id: Id, name: text }
        shape OrderCard from Order { total }
        query all_orders(org) -> OrderCard[] unscoped("admin: cross-org listing");
        "#);
    assert!(!ddl.contains(":ctx_org"), "scope must not inject:\n{ddl}");
    // the param `org` still filters, and soft-delete still guards.
    assert!(ddl.contains("`order`.`org_id` = :org"), "\n{ddl}");
    assert!(ddl.contains("`order`.`deleted_at` IS NULL"), "\n{ddl}");
}

#[test]
fn offset_pagination_and_with_count() {
    let ddl = gen(r#"
        Post { id: Id, title: text }
        shape PostShape from Post { title }
        query posts() -> PostShape[] {
          list Post order (id asc) page (50) offset with count;
        }
        "#);
    assert!(ddl.contains("LIMIT 50 OFFSET :offset"), "\n{ddl}");
    // second statement counts live rows, no LIMIT.
    assert!(ddl.contains("SELECT COUNT(*) AS `count`"), "\n{ddl}");
    assert!(ddl.contains("-- query posts (count)"), "\n{ddl}");
    // offset pagination is not keyset — no cursor comparison, no hidden columns.
    assert!(
        !ddl.contains("keyset"),
        "offset must not emit keyset:\n{ddl}"
    );
    assert!(!ddl.contains("__keyset"), "\n{ddl}");
}

#[test]
fn bare_model_return_projects_all_stored_columns() {
    let ddl = gen(r#"
        Org { id: Id, name: text }
        @sort(id asc)
        Order { id: Id, org: Org, status: text, total: int }
        query orders() -> Order[];
        "#);
    assert!(ddl.contains("`order`.`status` AS `status`"), "\n{ddl}");
    assert!(ddl.contains("`order`.`total` AS `total`"), "\n{ddl}");
    // forward relation projects its FK column
    assert!(ddl.contains("`order`.`org_id` AS `org`"), "\n{ddl}");
}

#[test]
fn zero_arg_filter_is_inlined_against_call_site() {
    let ddl = gen(r#"
        @sort(id asc)
        Product { id: Id, name: text, active: bool, stock: int }
        shape P from Product { name }
        filter sellable = active and stock > 0;
        query q() -> P[] { list Product where (sellable) order (name); }
        "#);
    // the bare filter atom expands to its body, resolved against Product.
    assert!(
        ddl.contains("`product`.`active` = TRUE AND `product`.`stock` > 0"),
        "\n{ddl}"
    );
}

#[test]
fn filter_call_substitutes_args_and_traverses_relation() {
    let ddl = gen(r#"
        City { id: Id, name: text }
        Address { id: Id, city: City }
        @sort(id asc)
        User { id: Id, address: Address, name: text }
        shape U from User { name }
        filter in_city(c) = address.city.name = $c;
        query users_in(c) -> U[] { list User where (in_city($c)) order (name); }
        "#);
    // `$c` (the filter param) is bound to the query's `$c` arg -> `:c`; the body's
    // relation path resolves through the call-site model's joins.
    assert!(
        ddl.contains("JOIN `address` AS `j_address` ON `j_address`.`id` = `user`.`address_id`"),
        "\n{ddl}"
    );
    assert!(ddl.contains("`j_address_city`.`name` = :c"), "\n{ddl}");
}

#[test]
fn recursive_filter_terminates_in_codegen() {
    // Mirrors the sema `recursive_filter_terminates` case: lowering must not loop.
    let ddl = gen(r#"
        @sort(id asc)
        Product { id: Id, name: text, active: bool }
        shape P from Product { name }
        filter loopy = active and loopy;
        query q() -> P[] { list Product where (loopy) order (name); }
        "#);
    assert!(ddl.contains("`product`.`active` = TRUE"), "\n{ddl}");
    assert!(ddl.contains("/* filter loopy recursion */"), "\n{ddl}");
}

#[test]
fn multi_hop_path_chains_joins() {
    let ddl = gen(r#"
        City { id: Id, name: text }
        Address { id: Id, city: City }
        @sort(id asc)
        User { id: Id, address: Address, name: text }
        shape UserCard from User { city = address.city.name }
        query user_by_id(id) -> UserCard;
        "#);
    // two chained joins, the second keyed off the first's alias.
    assert!(
        ddl.contains("JOIN `address` AS `j_address` ON `j_address`.`id` = `user`.`address_id`"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains(
            "JOIN `city` AS `j_address_city` ON `j_address_city`.`id` = `j_address`.`city_id`"
        ),
        "\n{ddl}"
    );
    assert!(ddl.contains("`j_address_city`.`name` AS `city`"), "\n{ddl}");
}

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

#[test]
fn sqlite_nested_to_many_orders_inside_json_group_array() {
    // SQLite's aggregate ORDER BY form (≥ 3.44): the sort rides inside
    // `json_group_array`, same cascade as the other dialects.
    let sql = gen_for(
        r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] }
        @sort(rank desc)
        OrderItem { id: Id, order: Order, rank: int, sku: text }
        shape OrderCard from Order { total, items { sku } }
        query order_by_id(id) -> OrderCard;
        "#,
        Dialect::Sqlite,
    );
    assert!(
        sql.contains("json_group_array(json_object('sku', `s1_order_item`.`sku`) ORDER BY `s1_order_item`.`rank` DESC)"),
        "\n{sql}"
    );
}

#[test]
fn pg_nested_to_many_uses_json_agg_and_double_quotes() {
    // Postgres uses `json_agg`/`json_build_object` + `'[]'::json` coalesce, double-quoted.
    let sql = gen_pg(
        r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] }
        @sort(id asc)
        OrderItem { id: Id, order: Order, quantity: int }
        shape OrderCard from Order { total, items { quantity } }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(
        sql.contains("COALESCE(json_agg(json_build_object('quantity', \"s1_order_item\".\"quantity\") ORDER BY \"s1_order_item\".\"id\" ASC), '[]'::json)"),
        "\n{sql}"
    );
    assert!(sql.contains("AS \"items[]\""), "\n{sql}");
}

#[test]
fn pg_nested_to_one_double_quotes_prefixed_alias() {
    let sql = gen_pg(
        r#"
        @sort(id asc)
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int }
        shape OrderCard from Order { total, placed_by { name } }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(
        sql.contains("\"j_placed_by\".\"name\" AS \"placed_by.name\""),
        "\n{sql}"
    );
}

// ---------- Postgres  -------------------------------------------------

#[test]
fn pg_select_double_quotes_identifiers_and_keeps_named_placeholders() {
    // Postgres double-quotes idents; the emitted template still carries `:name`
    // placeholders (the runtime rewrites them to `$n`), and the injected tombstone
    // uses the same `IS NULL` predicate.
    let sql = gen_pg(
        r#"
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp?, status: text, total: int }
        shape OrderCard from Order { status, total }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(sql.contains("FROM \"order\""), "\n{sql}");
    assert!(
        sql.contains("\"order\".\"status\" AS \"status\""),
        "\n{sql}"
    );
    assert!(
        sql.contains("WHERE \"order\".\"id\" = :id AND \"order\".\"deleted_at\" IS NULL"),
        "\n{sql}"
    );
    // no backtick-quoted identifiers in the statement body (the header has backticks).
    let body = &sql[sql.find("SELECT").unwrap()..];
    assert!(!body.contains('`'), "\n{sql}");
}

#[test]
fn pg_bare_bool_uses_true_keyword() {
    let sql = gen_pg(
        r#"
        @sort(id asc)
        Order { id: Id, active: bool, total: int }
        shape O from Order { total }
        query live() -> O[] { list Order where (active); }
        "#,
    );
    assert!(sql.contains("\"order\".\"active\" = TRUE"), "\n{sql}");
}

#[test]
fn pg_has_uses_jsonb_containment_operator() {
    // `has` is JSON-array containment: Postgres's `arr @> value`, not MySQL's
    // `value MEMBER OF(arr)`.
    let sql = gen_pg(
        r#"
        @sort(id asc)
        Order { id: Id, tags: text[], total: int }
        shape O from Order { total }
        query tagged(tag: text) -> O[] { list Order where (tags has $tag); }
        "#,
    );
    assert!(sql.contains("\"order\".\"tags\" @> :tag"), "\n{sql}");
    assert!(!sql.contains("MEMBER OF"), "\n{sql}");
}

#[test]
fn pg_join_double_quotes_alias_and_on() {
    let sql = gen_pg(
        r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        Order { id: Id, placed_by: User, total: int }
        shape OrderCard from Order { who = placed_by.name }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(
        sql.contains(
            "JOIN \"user\" AS \"j_placed_by\" ON \"j_placed_by\".\"id\" = \"order\".\"placed_by_id\" AND \"j_placed_by\".\"deleted_at\" IS NULL"
        ),
        "\n{sql}"
    );
}

// ---------- multi-scope DNF: per-callable alternative injection  -------

/// Extract one query's SELECT text from the `based gen sql` output (between its
/// `-- query <name>` header and the next header or EOF).
fn query_section<'a>(ddl: &'a str, name: &str) -> &'a str {
    let head = format!("-- query {name}\n");
    let start = ddl.find(&head).expect("query section present");
    let rest = &ddl[start..];
    match rest[head.len()..].find("\n-- query ") {
        Some(i) => &rest[..head.len() + i],
        None => rest,
    }
}

/// An OR model (`@scope Page` + `@scope Author`, two stacked decorators = two
/// alternatives): a query naming `Page` injects `page = $ctx.page`; a query naming
/// `Author` injects `author = $ctx.user`. The *same model* is filtered by a *different*
/// predicate per callable.
#[test]
fn or_scope_injects_the_callable_chosen_alternative() {
    let ddl = gen(r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
        }
        shape PostCard from Post { body }
        query posts_on_page() -> PostCard[] scoped Page   { list Post order (created desc); }
        query my_posts()      -> PostCard[] scoped Author { list Post order (created desc); }
        "#);
    let by_page = query_section(&ddl, "posts_on_page");
    let by_author = query_section(&ddl, "my_posts");
    // Each query injects only its chosen alternative's axis — never the other's.
    assert!(
        by_page.contains("WHERE `post`.`page_id` = :ctx_page"),
        "\n{by_page}"
    );
    assert!(!by_page.contains("author_id"), "\n{by_page}");
    assert!(
        by_author.contains("WHERE `post`.`author_id` = :ctx_user"),
        "\n{by_author}"
    );
    assert!(!by_author.contains("page_id"), "\n{by_author}");
}

/// An AND model (`@scope Page, Author`, one decorator = one two-axis alternative): a
/// callable naming both axes injects *both* equalities, ANDed, into the read `WHERE`.
#[test]
fn and_scope_injects_both_axes() {
    let ddl = gen(r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        @sort(created desc)
        Comment {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
        }
        shape CommentCard from Comment { body }
        query my_comments() -> CommentCard[] scoped Page, Author {
          list Comment order (created desc);
        }
        "#);
    let sec = query_section(&ddl, "my_comments");
    assert!(
        sec.contains("WHERE `comment`.`page_id` = :ctx_page AND `comment`.`author_id` = :ctx_user"),
        "\n{sec}"
    );
}

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

#[test]
fn raw_query_body_is_the_statement_with_bound_params() {
    // A whole-query raw body lowers verbatim: `${param}` → `:param`, `{table}` →
    // the target's quoted table, and NOTHING engine-built is injected — no
    // soft-delete tombstone, no ORDER BY, no LIMIT.
    let src = r#"
        @soft_delete(deleted_at)
        @sort(name asc)
        User { id: Id, deleted_at: timestamp?, name: text, total: int }
        shape UserRow from User { name }
        query heavy(min: int) -> UserRow[] {
          raw`SELECT u.name AS name FROM {table} u WHERE u.total >= ${min}`;
        }
        "#;
    for (dialect, table) in [
        (Dialect::MariaDb, "`user`"),
        (Dialect::Sqlite, "`user`"),
        (Dialect::Postgres, "\"user\""),
    ] {
        let out = gen_for(src, dialect);
        assert!(
            out.contains(&format!(
                "SELECT u.name AS name FROM {table} u WHERE u.total >= :min;"
            )),
            "\n{out}"
        );
        assert!(!out.contains("deleted_at"), "\n{out}");
        assert!(!out.contains("ORDER BY"), "\n{out}");
        assert!(!out.contains("LIMIT"), "\n{out}");
    }
}

#[test]
fn raw_query_trailing_semicolon_is_normalized() {
    // A raw body already ending in `;` emits exactly one terminator.
    let out = gen(r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query all() -> UserRow[] { raw`SELECT name FROM user;`; }
        "#);
    assert!(out.contains("SELECT name FROM user;\n"), "\n{out}");
    assert!(!out.contains(";;"), "\n{out}");
}

// ---------- aggregations + group by + having (T4) --------------------------

const AGG_SCHEMA: &str = r#"
    Buyer { id: Id, name: text }
    @soft_delete(deleted_at)
    Order {
      id: Id
      deleted_at: timestamp?
      buyer: Buyer
      total: decimal(12, 2)
      qty: int
    }
    shape BuyerStats from Order {
      who = buyer
      orders = count()
      revenue = sum(total)
      units = sum(qty)
      avg_qty = avg(qty)
      biggest = max(total)
    }
    query buyer_stats() -> BuyerStats[] {
      list Order group by (buyer) having (revenue > 100) order (revenue desc);
    }
"#;

#[test]
fn aggregate_query_groups_and_filters_soft_delete_first() {
    let sql = gen(AGG_SCHEMA);
    // count / sum(decimal) / min-max keep native form; sum(int) casts back on MariaDB.
    assert!(sql.contains("COUNT(*) AS `orders`"), "\n{sql}");
    assert!(sql.contains("SUM(`order`.`total`) AS `revenue`"), "\n{sql}");
    assert!(
        sql.contains("CAST(SUM(`order`.`qty`) AS SIGNED) AS `units`"),
        "\n{sql}"
    );
    assert!(
        sql.contains("CAST(AVG(`order`.`qty`) AS DOUBLE) AS `avg_qty`"),
        "\n{sql}"
    );
    assert!(sql.contains("MAX(`order`.`total`) AS `biggest`"), "\n{sql}");
    // soft-delete narrows rows before grouping.
    assert!(
        sql.contains("WHERE `order`.`deleted_at` IS NULL"),
        "\n{sql}"
    );
    assert!(sql.contains("GROUP BY `order`.`buyer_id`"), "\n{sql}");
    // HAVING inlines the aggregate expr (an alias isn't portable there).
    assert!(sql.contains("HAVING SUM(`order`.`total`) > 100"), "\n{sql}");
    assert!(
        sql.contains("ORDER BY SUM(`order`.`total`) DESC"),
        "\n{sql}"
    );
    // Never paginated / no count query.
    assert!(!sql.contains("LIMIT"), "\n{sql}");
}

#[test]
fn aggregate_query_postgres_casts() {
    let sql = gen_pg(AGG_SCHEMA);
    assert!(
        sql.contains("CAST(SUM(\"order\".\"qty\") AS BIGINT) AS \"units\""),
        "\n{sql}"
    );
    assert!(
        sql.contains("CAST(AVG(\"order\".\"qty\") AS DOUBLE PRECISION) AS \"avg_qty\""),
        "\n{sql}"
    );
    // sum(decimal) stays native numeric on Postgres (exact-string decode).
    assert!(
        sql.contains("SUM(\"order\".\"total\") AS \"revenue\""),
        "\n{sql}"
    );
    assert!(sql.contains("GROUP BY \"order\".\"buyer_id\""), "\n{sql}");
}

#[test]
fn aggregate_query_sqlite_casts_decimal_sum_to_text() {
    let sql = gen_for(AGG_SCHEMA, Dialect::Sqlite);
    // decimal sum → TEXT so it decodes as the wire string; int sum needs no cast.
    assert!(
        sql.contains("CAST(SUM(`order`.`total`) AS TEXT) AS `revenue`"),
        "\n{sql}"
    );
    assert!(sql.contains("SUM(`order`.`qty`) AS `units`"), "\n{sql}");
    assert!(
        sql.contains("CAST(AVG(`order`.`qty`) AS REAL) AS `avg_qty`"),
        "\n{sql}"
    );
}
