use super::*;

/// A to-one nested shape sub-object (`placed_by { name, email }`) returns a nested
/// JSON object end-to-end: the codegen-prefixed columns (`placed_by.name`, …) come back
/// from the live SELECT and the runtime reassembles them into a sub-object — proven
/// against a real engine, not compile-verified. Self-contained (no commerce schema) so
/// the nesting is the only variable.
#[tokio::test]
async fn nested_to_one_query_returns_nested_json() {
    let src = r#"
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, fulfilled_by: User?, total: int }
        shape OrderCard from Order { total, placed_by { name, email } }
        query order_by_id(id) -> OrderCard;
        query orders() -> OrderCard[];
    "#;
    let sf = parse_file(src, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != based_diagnostics::Severity::Error),
        "unexpected sema errors: {diags:#?}"
    );
    let c = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("generated DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `email`) VALUES ('u1', 'Ada', 'a@x.com');
            INSERT INTO `order` (`id`, `placed_by_id`, `total`) VALUES ('o1', 'u1', 500);
            "#,
        )
        .await
        .expect("seed");

    // `get`: the nested object rides back under `placed_by`, not as flat `placed_by.*`.
    let ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "total": 500, "placed_by": { "name": "Ada", "email": "a@x.com" } })
    );

    // `list`: every row reassembles independently.
    let listed = dispatch(
        &c,
        &backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/orders",
        json!({}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([{ "total": 500, "placed_by": { "name": "Ada", "email": "a@x.com" } }])
    );
}

/// An optional to-one nest whose row is absent comes back as JSON `null` — never an
/// object of nulls (which a typed client cannot decode into `Option<Shape>`) — while a
/// present row nests normally and sheds the internal presence probe. Proven at both
/// levels: a top-level nest (flat-column reassembly) and a nest inside a to-many JSON
/// aggregate (SQL-built objects).
#[tokio::test]
async fn absent_optional_to_one_nest_is_json_null() {
    let src = r#"
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order {
          id: Id
          placed_by:    User
          fulfilled_by: User?
          total:        int
          items:        Item[] (Item.order)
        }
        @sort(id asc)
        Item { id: Id, order: Order, checker: User?, qty: int, @index order }
        shape OrderCard from Order {
          total
          placed_by { name }
          fulfilled_by { name, email }
          items { qty, checker { name } }
        }
        query order_by_id(id) -> OrderCard;
    "#;
    let sf = parse_file(src, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != based_diagnostics::Severity::Error),
        "unexpected sema errors: {diags:#?}"
    );
    let c = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend.execute_batch(&ddl).await.unwrap_or_else(|e| {
        panic!(
            "generated DDL failed: {e:?}
{ddl}"
        )
    });
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `email`) VALUES ('u1', 'Ada', 'a@x.com');
            INSERT INTO `order` (`id`, `placed_by_id`, `fulfilled_by_id`, `total`)
                VALUES ('o1', 'u1', NULL, 500), ('o2', 'u1', 'u1', 700);
            INSERT INTO `item` (`id`, `order_id`, `checker_id`, `qty`)
                VALUES ('i1', 'o1', NULL, 3), ('i2', 'o1', 'u1', 4);
            "#,
        )
        .await
        .expect("seed");

    let ids = SeqIdGen::default();
    let absent = dispatch(
        &c,
        &backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(absent.status, 200, "{:?}", absent.body);
    assert_eq!(
        absent.body,
        json!({
            "total": 500,
            "placed_by": { "name": "Ada" },
            "fulfilled_by": null,
            "items": [
                { "qty": 3, "checker": null },
                { "qty": 4, "checker": { "name": "Ada" } },
            ],
        })
    );

    let present = dispatch(
        &c,
        &backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o2" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(present.status, 200, "{:?}", present.body);
    assert_eq!(
        present.body["fulfilled_by"],
        json!({ "name": "Ada", "email": "a@x.com" }),
        "a matched optional nest sheds the presence probe"
    );
}
