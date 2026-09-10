use super::*;

// ---------- computed shape fields (per-row derived scalars, D121) ----------

const COMPUTED_SCHEMA: &str = r#"
    Product {
      id:       Id
      name:     text
      brand:    text
      price:    int
      discount: int
      status:   text
    }
    shape ProductCard from Product {
      name
      net   = price - discount
      label = brand || " " || name
      tier  = case when price > 100 then "premium" else "standard" end
    }
    query product_card(id) -> ProductCard;
"#;

#[test]
fn computed_arith_concat_case_lower_per_dialect() {
    let pg = gen_pg(COMPUTED_SCHEMA);
    // Arithmetic: a parenthesized SQL expression over the row's own columns.
    assert!(
        pg.contains(r#"("product"."price" - "product"."discount") AS "net""#),
        "\n{pg}"
    );
    // Concat: Postgres uses `||`.
    assert!(
        pg.contains(r#"("product"."brand" || ' ' || "product"."name") AS "label""#),
        "\n{pg}"
    );
    // Case: reuses the shared predicate lowering for the `when`.
    assert!(
        pg.contains(
            r#"CASE WHEN "product"."price" > 100 THEN 'premium' ELSE 'standard' END AS "tier""#
        ),
        "\n{pg}"
    );
}

#[test]
fn computed_concat_uses_concat_fn_on_mysql_family() {
    let maria = gen(COMPUTED_SCHEMA);
    // The MySQL/MariaDB family has no string `||`, so concat lowers to CONCAT(…).
    assert!(
        maria.contains("CONCAT(`product`.`brand`, ' ', `product`.`name`) AS `label`"),
        "\n{maria}"
    );
    // Arithmetic still parenthesizes.
    assert!(
        maria.contains("(`product`.`price` - `product`.`discount`) AS `net`"),
        "\n{maria}"
    );
}
