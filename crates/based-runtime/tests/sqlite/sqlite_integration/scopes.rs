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

const COMPOSITION: &str = r#"
Org { id: Id, name: text }
User { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)
scope Author (author: User = $ctx.user)
@scope Tenant
@scope Author
Post { id: Id, org: Org, author: User, title: text, @index(org), @index(author) }
@scope Tenant, Author
Comment { id: Id, org: Org, author: User, title: text, @index(org, author) }
shape PostRow from Post { title }
shape CommentRow from Comment { title }
query by_tenant() -> PostRow[] scoped Tenant { list Post order (title); }
query by_author() -> PostRow[] scoped Author { list Post order (title); }
query both() -> PostRow[] scoped Tenant, Author { list Post order (title); }
query comments() -> CommentRow[] scoped Tenant, Author { list Comment order (title); }
mutation retitle(id: Id, title: text) -> ok scoped Tenant, Author {
 update Comment where (id = $id) { title = $title };
}
"#;

async fn composition_backend() -> (Compiled, SqliteBackend) {
    let c = compile_sqlite(COMPOSITION);
    let b = SqliteBackend::in_memory().unwrap();
    b.execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .unwrap();
    b.execute_batch("INSERT INTO org (id, name) VALUES ('a','A'), ('b','B');\nINSERT INTO user (id, name) VALUES ('u','U'), ('v','V');").await.unwrap();
    for table in ["post", "comment"] {
        b.execute_batch(&format!("INSERT INTO {table} (id, org_id, author_id, title) VALUES ('aa','a','u','AA'), ('ab','a','v','AB'), ('ba','b','u','BA'), ('bb','b','v','BB');")).await.unwrap();
    }
    (c, b)
}

#[tokio::test]
async fn scope_alternatives_select_one_axis_and_named_axes_intersect() {
    let (c, b) = composition_backend().await;
    for (route, titles) in [
        ("by_tenant", vec!["AA", "AB"]),
        ("by_author", vec!["AA", "BA"]),
        ("both", vec!["AA"]),
        ("comments", vec!["AA"]),
    ] {
        let response = call(
            &c,
            &b,
            "POST",
            &format!("/q/{route}"),
            json!({}),
            json!({"org":"a", "user":"u"}),
        )
        .await;
        assert_eq!(response.status, 200, "{:?}", response.body);
        assert_eq!(
            response.body,
            json!(titles
                .iter()
                .map(|title| json!({"title":title}))
                .collect::<Vec<_>>())
        );
    }
    let missing = call(&c, &b, "POST", "/q/comments", json!({}), json!({"org":"a"})).await;
    assert_eq!(missing.status, 400);
    assert_eq!(missing.body["error"]["code"], "missing_ctx");
}

#[tokio::test]
async fn scoped_update_cannot_change_either_foreign_axis() {
    let (c, b) = composition_backend().await;
    for id in ["ab", "ba", "bb"] {
        let response = call(
            &c,
            &b,
            "POST",
            "/m/retitle",
            json!({"id":id,"title":"stolen"}),
            json!({"org":"a","user":"u"}),
        )
        .await;
        assert_eq!(response.status, 404, "{:?}", response.body);
    }
    let response = call(
        &c,
        &b,
        "POST",
        "/m/retitle",
        json!({"id":"aa","title":"changed"}),
        json!({"org":"a","user":"u"}),
    )
    .await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    let mut db = b.checkout("").await.unwrap();
    let rows = based_runtime::fetch_all(db.fetch("SELECT title FROM comment ORDER BY id", &[]))
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|r| r["title"].clone()).collect::<Vec<_>>(),
        json!(["changed", "AB", "BA", "BB"])
            .as_array()
            .unwrap()
            .clone()
    );
}

#[tokio::test]
async fn raw_leaves_keep_root_scope_but_raw_subqueries_and_bodies_own_their_filters() {
    let c = compile_sqlite(
        r#"
Org { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)
@scope Tenant
Note { id: Id, org: Org, title: text, @index(org) }
shape NoteRow from Note { title }
shape CountRow from Note { title, count = raw`(SELECT COUNT(*) FROM note)` }
query leaf() -> NoteRow[] scoped Tenant { list Note where (raw`1 = 1 OR 1 = 1`) order (title); }
query counts() -> CountRow[] scoped Tenant { list Note order (title); }
query escape() -> NoteRow[] unscoped("admin inventory across tenants") { raw`SELECT title FROM note ORDER BY title`; }
"#,
    );
    let b = SqliteBackend::in_memory().unwrap();
    b.execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .unwrap();
    b.execute_batch("INSERT INTO org (id,name) VALUES ('a','A'),('b','B'); INSERT INTO note (id,org_id,title) VALUES ('n1','a','A secret'),('n2','b','B secret');").await.unwrap();
    let ctx = json!({"org":"a"});
    let leaf = call(&c, &b, "POST", "/q/leaf", json!({}), ctx.clone()).await;
    assert_eq!(leaf.status, 200, "{:?}", leaf.body);
    assert_eq!(leaf.body, json!([{"title":"A secret"}]));
    let counts = call(&c, &b, "POST", "/q/counts", json!({}), ctx.clone()).await;
    assert_eq!(counts.status, 200, "{:?}", counts.body);
    assert_eq!(counts.body, json!([{"title":"A secret", "count":2}]));
    let escape = call(&c, &b, "POST", "/q/escape", json!({}), ctx).await;
    assert_eq!(escape.status, 200, "{:?}", escape.body);
    assert_eq!(
        escape.body,
        json!([{"title":"A secret"},{"title":"B secret"}])
    );
}
