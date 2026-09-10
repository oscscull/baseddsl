use super::*;

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
fn optional_ctx_read_is_null_safe() {
    // `$ctx.user?` is an optional context read (auth.md Handle 1): an absent field binds SQL
    // NULL, so the `author =` leaf lowers to null-safe equality (`<=>`), not a present-guard.
    // An anonymous caller (no `user`) matches only the rows whose own `author` is unset
    // (`author IS NULL`) — the leaf never widens to TRUE, so the public-visibility guard on
    // the other side of the `or` still gates the private rows out.
    let ddl = gen(r#"
        @sort(id asc)
        User { id: Id, name: text }
        Post { id: Id, author: User, visibility: text, body: text }
        shape PostCard from Post { id, body }
        query feed() -> PostCard[] {
          list Post where (author = $ctx.user? or visibility = "public");
        }
        "#);
    assert!(
        ddl.contains("`post`.`author_id` <=> :ctx_user"),
        "optional ctx `=` must lower to null-safe equality\n{ddl}"
    );
    assert!(
        !ddl.contains("__present"),
        "a ctx read is null-driven, not present-guarded\n{ddl}"
    );
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
