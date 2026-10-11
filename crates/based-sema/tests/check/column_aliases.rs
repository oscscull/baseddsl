use super::*;

const ORDER: &str = "@key(tenant_id, id)\nOrder { tenant_id: int  id: text  total: int }\nCustomer { id: Id  name: text }\n";

fn line(field: &str) -> (CheckedSchema, Vec<Diagnostic>) {
    analyze(&format!("{ORDER}Line {{ id: Id  qty: int  {field} }}"))
}

fn line_errors(field: &str) -> Vec<String> {
    let (_, d) = line(field);
    errors(&d).into_iter().map(str::to_owned).collect()
}

#[test]
fn per_part_map_resolves_in_key_order() {
    let (schema, d) = line(r#"order: Order? (column id = "ord no", tenant_id = "ord_tenant")"#);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let model = schema.model("Line").unwrap();
    let pairs: Vec<(String, String)> = schema
        .fk_columns(model.member("order").unwrap())
        .into_iter()
        .map(|(column, part)| (column, part.physical_col().to_owned()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("ord_tenant".to_owned(), "tenant_id".to_owned()),
            ("ord no".to_owned(), "id".to_owned())
        ]
    );
}

#[test]
fn malformed_maps_are_errors() {
    for field in [
        r#"order: Order (column tenant_id = "a", number = "b")"#,
        r#"order: Order (column tenant_id = "a", tenant_id = "b", id = "c")"#,
        r#"order: Order (column tenant_id = "a")"#,
        r#"order: Order (column "ord_ref")"#,
        r#"customer: Customer (column id = "cust_ref")"#,
        r#"note: text (column id = "n")"#,
    ] {
        assert_eq!(line_errors(field), ["E0128"], "{field}");
    }
    // The single-name form stays the spelling for a single-column relation.
    assert!(line_errors(r#"customer: Customer (column "cust_ref")"#).is_empty());
}

#[test]
fn aliased_columns_must_not_collide() {
    for field in [
        r#"order: Order (column tenant_id = "x", id = "x")"#,
        r#"order: Order (column tenant_id = "qty", id = "b")"#,
        r#"order: Order (column tenant_id = "a", id = "b")  other: Order (column tenant_id = "a", id = "c")"#,
        r#"order: Order (column tenant_id = "customer_id", id = "b")  customer: Customer"#,
        r#"order: Order (column tenant_id = "other_tenant_id", id = "b")  other: Order"#,
    ] {
        assert!(line_errors(field).contains(&"E0129".to_owned()), "{field}");
    }
}
