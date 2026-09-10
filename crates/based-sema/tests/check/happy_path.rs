use super::*;

// ---------- happy path -----------------------------------------------------

#[test]
fn minimal_schema_is_clean() {
    assert_clean(
        r#"
        @soft_delete(deleted_at)
        @sort(name asc)
        Org {
          id: Id
          deleted_at: timestamp?
          name: text
          slug: text (unique)
        }
        shape OrgCard from Org { name, slug }
        query org_by_id(id) -> OrgCard;
        query orgs() -> OrgCard[];
        "#,
    );
}

#[test]
fn resolved_schema_shape() {
    let (schema, diags) = analyze(
        r#"
        @soft_delete(deleted_at)
        @created(created_at)
        Order {
          id: Id
          deleted_at: timestamp?
          created_at: timestamp
          placed_by: User
          items: OrderItem[]
        }
        OrderItem { id: Id, order: Order, qty: int }
        User {
          id: Id
          name: text
          placed_orders: Order[] (Order.placed_by)
        }
        "#,
    );
    assert!(
        errors(&diags).is_empty(),
        "unexpected errors: {:?}",
        codes(&diags)
    );

    let order = schema.model("Order").expect("Order resolved");
    assert_eq!(order.table, "order"); // snake_case, no pluralization
    assert_eq!(order.created.as_deref(), Some("created_at"));
    assert!(matches!(
        order.soft_delete.as_ref().unwrap().mode,
        SoftMode::Timestamp
    ));
    // implicit `id` is prepended
    assert!(matches!(
        order.member("id").unwrap().kind,
        MemberKind::Scalar { .. }
    ));
    // forward relation -> FK column `placed_by_id`
    match &order.member("placed_by").unwrap().kind {
        MemberKind::Forward { fk_col, target, .. } => {
            assert_eq!(fk_col, "placed_by_id");
            assert_eq!(target, "User");
        }
        k => panic!("expected forward relation, got {k:?}"),
    }
    // to-many `items` with no explicit ref infers the inverse via OrderItem.order
    match &order.member("items").unwrap().kind {
        MemberKind::Inverse { via, target } => {
            assert_eq!(via, "order");
            assert_eq!(target, "OrderItem");
        }
        k => panic!("expected inferred inverse, got {k:?}"),
    }

    assert_eq!(schema.model("OrderItem").unwrap().table, "order_item");
}

#[test]
fn query_verb_inference() {
    let (schema, diags) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserCard from User { name }
        query user_by_id(id) -> UserCard;
        query users() -> UserCard[];
        "#,
    );
    assert!(errors(&diags).is_empty(), "{:?}", codes(&diags));
    let get = schema
        .queries
        .iter()
        .find(|q| q.name == "user_by_id")
        .unwrap();
    assert_eq!(get.verb, Verb::Get);
    assert_eq!(get.target, "User");
    assert_eq!(get.ret_shape.as_deref(), Some("UserCard"));
    let list = schema.queries.iter().find(|q| q.name == "users").unwrap();
    assert_eq!(list.verb, Verb::List);
    assert!(list.many);
}
