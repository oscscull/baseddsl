use super::*;

#[test]
fn for_update_emits_lock_clause_per_dialect() {
    // `for update` appends the row-locking clause last (after ORDER BY / LIMIT). Postgres
    // and the MySQL/MariaDB family emit `FOR UPDATE`; SQLite emits nothing (whole-database
    // locking on the transaction already serializes writers).
    let src = r#"
        Product { id: Id, sku: text, name: text, price: int }
        shape ProductRow from Product { sku, name, price }
        query product_for_update(id) -> ProductRow {
            get Product where (id = $id) for update;
        }
        "#;
    let maria = gen(src);
    assert!(maria.trim_end().ends_with("FOR UPDATE;"), "\n{maria}");
    let pg = gen_pg(src);
    assert!(pg.trim_end().ends_with("FOR UPDATE;"), "\n{pg}");
    let sqlite = gen_for(src, Dialect::Sqlite);
    assert!(!sqlite.contains("FOR UPDATE"), "\n{sqlite}");
    // The lock rides after the WHERE, not before it.
    assert!(
        pg.contains("WHERE \"product\".\"id\" = :id\nFOR UPDATE;"),
        "\n{pg}"
    );
}

#[test]
fn for_update_list_lock_follows_order_and_limit() {
    // On a `list … for update` the lock clause comes strictly last — after ORDER BY and LIMIT.
    let src = r#"
        Product { id: Id, sku: text, price: int }
        shape ProductRow from Product { sku, price }
        query cheap_for_update(max) -> ProductRow[] {
            list Product where (price <= $max) order (price) page (20) offset for update;
        }
        "#;
    let pg = gen_pg(src);
    assert!(
        pg.contains("ORDER BY \"product\".\"price\" ASC\nLIMIT 20 OFFSET :offset\nFOR UPDATE;"),
        "\n{pg}"
    );
    assert!(!gen_for(src, Dialect::Sqlite).contains("FOR UPDATE"));
}

#[test]
fn for_update_wait_modes_emit_nowait_and_skip_locked() {
    // `for update nowait` / `for update skip locked` append the wait mode after `FOR UPDATE`
    // on Postgres and the MySQL/MariaDB family; SQLite is a no-op for every mode.
    let src = r#"
        Product { id: Id, sku: text, name: text, price: int }
        shape ProductRow from Product { sku, name, price }
        query lock_nowait(id) -> ProductRow {
            get Product where (id = $id) for update nowait;
        }
        query lock_skip(max) -> ProductRow[] {
            list Product where (price <= $max) order (price) for update skip locked;
        }
        "#;
    let pg = gen_pg(src);
    assert!(pg.contains("\nFOR UPDATE NOWAIT;"), "\n{pg}");
    assert!(pg.contains("\nFOR UPDATE SKIP LOCKED;"), "\n{pg}");
    let maria = gen(src);
    assert!(maria.contains("\nFOR UPDATE NOWAIT;"), "\n{maria}");
    assert!(maria.contains("\nFOR UPDATE SKIP LOCKED;"), "\n{maria}");
    let sqlite = gen_for(src, Dialect::Sqlite);
    assert!(!sqlite.contains("FOR UPDATE"), "\n{sqlite}");
    assert!(!sqlite.contains("NOWAIT"), "\n{sqlite}");
    assert!(!sqlite.contains("SKIP LOCKED"), "\n{sqlite}");
}

#[test]
fn sqlite_nested_to_many_orders_inside_json_group_array() {
    // SQLite's aggregate ORDER BY form (≥ 3.44): the sort rides inside
    // `json_group_array`, same cascade as the other dialects.
    let sql = gen_for(
        r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] }
        @sort(rank desc)
        OrderItem { id: Id, order: Order, rank: int, sku: text }
        shape OrderCard from Order { total, items { sku } }
        query order_by_id(id) -> OrderCard;
        "#,
        Dialect::Sqlite,
    );
    assert!(
        sql.contains("json_group_array(json_object('sku', `s1_order_item`.`sku`) ORDER BY `s1_order_item`.`rank` DESC)"),
        "\n{sql}"
    );
}

#[test]
fn pg_nested_to_many_uses_json_agg_and_double_quotes() {
    // Postgres uses `json_agg`/`json_build_object` + `'[]'::json` coalesce, double-quoted.
    let sql = gen_pg(
        r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] }
        @sort(id asc)
        OrderItem { id: Id, order: Order, quantity: int }
        shape OrderCard from Order { total, items { quantity } }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(
        sql.contains("COALESCE(json_agg(json_build_object('quantity', \"s1_order_item\".\"quantity\") ORDER BY \"s1_order_item\".\"id\" ASC), '[]'::json)"),
        "\n{sql}"
    );
    assert!(sql.contains("AS \"items[]\""), "\n{sql}");
}

#[test]
fn pg_nested_to_one_double_quotes_prefixed_alias() {
    let sql = gen_pg(
        r#"
        @sort(id asc)
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int }
        shape OrderCard from Order { total, placed_by { name } }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(
        sql.contains("\"j_placed_by\".\"name\" AS \"placed_by.name\""),
        "\n{sql}"
    );
}
