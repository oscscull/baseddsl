use super::*;

/// A scoped child reached **only** through a nested shape sub-object is confined by its
/// `@scope`, so a cross-scope nested read can't leak rows the caller's `$ctx` excludes —
/// mirroring D34's Ticket→Contact but through nests. Here the parent `Order` is scoped on
/// `Tenant` (org) and its nested children are scoped on a *divergent* axis (`Region`): a
/// to-one `contact { name }` and a to-many `items { sku }`. Against a live engine, an
/// out-of-scope contact's name reads back NULL (not the real name) and an out-of-scope
/// line item is absent from the array — proven, not compile-only.
#[tokio::test]
async fn nest_reached_scoped_child_is_confined_cross_tenant() {
    let c = compile_sqlite(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        @sort(id asc)
        Order { id: Id, org: Org, contact: Contact?, total: int, items: LineItem[] }
        @scope Region
        @sort(id asc)
        Contact { id: Id, region: Region, name: text }
        @scope Region
        @sort(id asc)
        LineItem { id: Id, order: Order, region: Region, sku: text }
        shape OrderCard from Order { total, contact { name }, items { sku } }
        query order_by_id(id) -> OrderCard scoped Tenant, Region;
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`) VALUES ('org-1', 'Acme');
            INSERT INTO `region` (`id`, `name`) VALUES ('r1', 'North'), ('r2', 'South');
            INSERT INTO `contact` (`id`, `region_id`, `name`)
                VALUES ('c1', 'r1', 'InRegion'), ('c2', 'r2', 'OutRegion');
            INSERT INTO `order` (`id`, `org_id`, `contact_id`, `total`)
                VALUES ('o1', 'org-1', 'c1', 100), ('o2', 'org-1', 'c2', 200);
            INSERT INTO `line_item` (`id`, `order_id`, `region_id`, `sku`)
                VALUES ('li1', 'o1', 'r1', 'IN'), ('li2', 'o1', 'r2', 'OUT');
            "#,
        )
        .await
        .expect("seed");

    let ctx = json!({ "org": "org-1", "region": "r1" });

    // o1: contact c1 is in-region → its name reads back; item li1 is in-region, li2 is
    // out-of-region → only li1 survives the correlated subquery's scope predicate.
    let o1 = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        ctx.clone(),
    )
    .await;
    assert_eq!(o1.status, 200, "{:?}", o1.body);
    assert_eq!(
        o1.body,
        json!({ "total": 100, "contact": { "name": "InRegion" }, "items": [{ "sku": "IN" }] })
    );

    // o2: contact c2 is out-of-region → the nest join finds no in-scope row, so the
    // whole nest reads back as JSON null (an absent optional to-one) instead of
    // leaking "OutRegion"; o2 has no in-region items.
    let o2 = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o2" }),
        ctx,
    )
    .await;
    assert_eq!(o2.status, 200, "{:?}", o2.body);
    assert_eq!(
        o2.body,
        json!({ "total": 200, "contact": null, "items": [] }),
        "cross-scope nested read must not leak the out-of-region contact"
    );
}
