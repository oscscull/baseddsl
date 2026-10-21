use super::*;

// `{id}` expands to the row model's physical key column, not a literal `id`.

#[test]
fn raw_id_is_a_custom_key_column() {
    let src = r#"
        @key(sku)
        Product { sku: text, name: text }
        shape P from Product { name, k = raw`{id}` }
        filter keyed = raw`{id} <> ''`;
        query by_where() -> P[] { list Product where (raw`{id} IS NOT NULL`) order (name); }
        query by_filter() -> P[] { list Product where (keyed) order (name); }
        query whole() -> P[] { raw`SELECT name, {id} AS k FROM {table}`; }
        "#;
    let out = gen(src);
    let where_q = query_section(&out, "by_where");
    assert!(
        where_q.contains("(`product`.`sku` IS NOT NULL)"),
        "\n{where_q}"
    );
    assert!(where_q.contains("(`product`.`sku`) AS `k`"), "\n{where_q}");
    let filter_q = query_section(&out, "by_filter");
    assert!(filter_q.contains("(`product`.`sku` <> '')"), "\n{filter_q}");
    let whole_q = query_section(&out, "whole");
    assert!(
        whole_q.contains("SELECT name, `product`.`sku` AS k FROM `product`;"),
        "\n{whole_q}"
    );
    assert!(!out.contains("`product`.`id`"), "\n{out}");
}

#[test]
fn raw_id_is_a_renamed_id_column() {
    let src = r#"
        User { id: Id (column "user_pk"), name: text }
        shape U from User { name }
        query q() -> U[] { list User where (raw`{id} > 0`) order (name); }
        "#;
    for (dialect, col) in [
        (Dialect::MariaDb, "`user`.`user_pk`"),
        (Dialect::Postgres, "\"user\".\"user_pk\""),
    ] {
        let out = gen_for(src, dialect);
        assert!(out.contains(&format!("({col} > 0)")), "\n{out}");
    }
}
