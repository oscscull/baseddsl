//! DML (query -> SELECT) codegen tests. Parse + check a whole-schema snippet, then
//! assert on the generated SELECT text. The headline assertions are the soft-delete
//! injection (root `WHERE` + every join `ON`) and the sort/pagination cascade.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

fn gen(src: &str) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    sql::dml::dml(&schema, &sf.decls, Dialect::MariaDb)
}

#[test]
fn bare_get_injects_soft_delete_and_maps_param() {
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text, total: int }
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
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, total: int }
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
        User { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order { deleted_at: timestamp?, placed_by: User, placed_at: timestamp }
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
fn optional_relation_is_left_join() {
    let ddl = gen(r#"
        User { name: text }
        @sort(id asc)
        Order { fulfilled_by: User?, total: int }
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
        User { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        @sort(created_at desc)
        Post { deleted_at: timestamp?, author: User, created_at: timestamp }
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
          deleted_at: timestamp?
          created_at: timestamp
          org: Org
          active: bool (default true)
        }
        Org { name: text }
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
}

#[test]
fn scope_predicate_is_injected() {
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        @scope(org = $ctx.org)
        @sort(id asc)
        Order { deleted_at: timestamp?, org: Org, total: int }
        Org { name: text }
        shape OrderCard from Order { total }
        query orders() -> OrderCard[];
        "#);
    // @scope rides the same injection path; `$ctx.org` -> `:ctx_org`.
    assert!(ddl.contains("`order`.`org_id` = :ctx_org"), "\n{ddl}");
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
}

#[test]
fn bare_model_return_projects_all_stored_columns() {
    let ddl = gen(r#"
        Org { name: text }
        @sort(id asc)
        Order { org: Org, status: text, total: int }
        query orders() -> Order[];
        "#);
    assert!(ddl.contains("`order`.`status` AS `status`"), "\n{ddl}");
    assert!(ddl.contains("`order`.`total` AS `total`"), "\n{ddl}");
    // forward relation projects its FK column
    assert!(ddl.contains("`order`.`org_id` AS `org`"), "\n{ddl}");
}

#[test]
fn multi_hop_path_chains_joins() {
    let ddl = gen(r#"
        City { name: text }
        Address { city: City }
        @sort(id asc)
        User { address: Address, name: text }
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
