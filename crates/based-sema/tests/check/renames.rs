use super::*;

// ---------- @was rename directive -------------------------------------------

#[test]
fn field_was_rename_is_clean() {
    // `@was("upc")` on a field whose old column is gone from the model — a valid rename.
    assert_clean(
        r#"
        @sort(name asc)
        Product {
          id: Id
          name: text
          barcode: text? @was("upc")
        }
        shape ProductCard from Product { name }
        query products() -> ProductCard[];
        "#,
    );
}

#[test]
fn model_was_rename_is_clean() {
    assert_clean(
        r#"
        @sort(name asc)
        @was("legacy_product")
        Product {
          id: Id
          name: text
        }
        shape ProductCard from Product { name }
        query products() -> ProductCard[];
        "#,
    );
}

#[test]
fn field_was_naming_a_live_column_is_e0191() {
    // `barcode @was("sku")` while `sku` still exists — can't be the rename source.
    let (_, d) = analyze(
        r#"
        Product {
          id: Id
          sku: text
          barcode: text? @was("sku")
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0191"), "{:?}", codes(&d));
}

#[test]
fn field_was_naming_itself_is_e0190() {
    let (_, d) = analyze(
        r#"
        Product {
          id: Id
          barcode: text? @was("barcode")
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0190"), "{:?}", codes(&d));
}

#[test]
fn model_was_naming_a_live_table_is_e0191() {
    let (_, d) = analyze(
        r#"
        Legacy { id: Id, name: text }
        @was("legacy")
        Product { id: Id, name: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0191"), "{:?}", codes(&d));
}

#[test]
fn model_was_naming_its_own_table_is_e0190() {
    let (_, d) = analyze(
        r#"
        @was("product")
        Product { id: Id, name: text }
        "#,
    );
    assert!(errors(&d).contains(&"E0190"), "{:?}", codes(&d));
}
