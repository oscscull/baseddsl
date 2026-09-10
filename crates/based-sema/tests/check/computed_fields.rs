use super::*;

// ---------- computed shape fields (per-row derived scalars, D121) -----------

const COMPUTED_MODELS: &str = r#"
        City { id: Id, name: text }
        Product {
          id:       Id
          name:     text
          brand:    text
          price:    int
          discount: int
          note:     text
          made_in:  City
        }
"#;

fn computed(body: &str) -> Vec<Diagnostic> {
    let mut src = COMPUTED_MODELS.to_string();
    src.push_str(body);
    analyze(&src).1
}

#[test]
fn computed_arith_concat_case_is_clean() {
    let mut src = COMPUTED_MODELS.to_string();
    src.push_str(
        r#"
        shape ProductCard from Product {
          name
          net   = price - discount
          gross = price * 2 + discount
          label = brand || " " || name
          tier  = case when price > 100 then "premium" else "standard" end
          city  = case when price > 0 then made_in.name else "unknown" end
        }
        query product_card(id) -> ProductCard;
        "#,
    );
    assert_clean(&src);
}

#[test]
fn arith_over_text_column_is_e0320() {
    let d = computed(
        r#"
        shape Bad from Product { x = name + price }
        query bad(id) -> Bad;
        "#,
    );
    assert!(errors(&d).contains(&"E0320"), "{:?}", codes(&d));
}

#[test]
fn concat_of_numeric_column_is_e0321() {
    let d = computed(
        r#"
        shape Bad from Product { x = name || price }
        query bad(id) -> Bad;
        "#,
    );
    assert!(errors(&d).contains(&"E0321"), "{:?}", codes(&d));
}

#[test]
fn case_branches_of_different_types_is_e0322() {
    let d = computed(
        r#"
        shape Bad from Product { x = case when price > 0 then name else price end }
        query bad(id) -> Bad;
        "#,
    );
    assert!(errors(&d).contains(&"E0322"), "{:?}", codes(&d));
}

#[test]
fn param_operand_in_computed_field_is_e0323() {
    let d = computed(
        r#"
        shape Bad from Product { x = price + $bump }
        query bad(id) -> Bad;
        "#,
    );
    assert!(errors(&d).contains(&"E0323"), "{:?}", codes(&d));
}

#[test]
fn computed_field_alongside_aggregate_is_e0324() {
    let d = computed(
        r#"
        shape Bad from Product { brand, orders = count(), net = price - discount }
        query bad() -> Bad[] { list Product group by (brand); }
        "#,
    );
    assert!(errors(&d).contains(&"E0324"), "{:?}", codes(&d));
}
