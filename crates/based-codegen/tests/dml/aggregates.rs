use super::*;

// ---------- aggregations + group by + having (T4) --------------------------

const AGG_SCHEMA: &str = r#"
    Buyer { id: Id, name: text }
    @soft_delete(deleted_at)
    Order {
      id: Id
      deleted_at: timestamp?
      buyer: Buyer
      total: decimal(12, 2)
      qty: int
    }
    shape BuyerStats from Order {
      who = buyer
      orders = count()
      revenue = sum(total)
      units = sum(qty)
      avg_qty = avg(qty)
      biggest = max(total)
    }
    query buyer_stats() -> BuyerStats[] {
      list Order group by (buyer) having (revenue > 100) order (revenue desc);
    }
"#;

#[test]
fn aggregate_query_groups_and_filters_soft_delete_first() {
    let sql = gen(AGG_SCHEMA);
    // count / sum(decimal) / min-max keep native form; sum(int) casts back on MariaDB.
    assert!(sql.contains("COUNT(*) AS `orders`"), "\n{sql}");
    assert!(sql.contains("SUM(`order`.`total`) AS `revenue`"), "\n{sql}");
    assert!(
        sql.contains("CAST(SUM(`order`.`qty`) AS SIGNED) AS `units`"),
        "\n{sql}"
    );
    assert!(
        sql.contains("CAST(AVG(`order`.`qty`) AS DOUBLE) AS `avg_qty`"),
        "\n{sql}"
    );
    assert!(sql.contains("MAX(`order`.`total`) AS `biggest`"), "\n{sql}");
    // soft-delete narrows rows before grouping.
    assert!(
        sql.contains("WHERE `order`.`deleted_at` IS NULL"),
        "\n{sql}"
    );
    assert!(sql.contains("GROUP BY `order`.`buyer_id`"), "\n{sql}");
    // HAVING inlines the aggregate expr (an alias isn't portable there).
    assert!(sql.contains("HAVING SUM(`order`.`total`) > 100"), "\n{sql}");
    assert!(
        sql.contains("ORDER BY SUM(`order`.`total`) DESC"),
        "\n{sql}"
    );
    // Never paginated / no count query.
    assert!(!sql.contains("LIMIT"), "\n{sql}");
}

#[test]
fn aggregate_query_postgres_casts() {
    let sql = gen_pg(AGG_SCHEMA);
    assert!(
        sql.contains("CAST(SUM(\"order\".\"qty\") AS BIGINT) AS \"units\""),
        "\n{sql}"
    );
    assert!(
        sql.contains("CAST(AVG(\"order\".\"qty\") AS DOUBLE PRECISION) AS \"avg_qty\""),
        "\n{sql}"
    );
    // sum(decimal) stays native numeric on Postgres (exact-string decode).
    assert!(
        sql.contains("SUM(\"order\".\"total\") AS \"revenue\""),
        "\n{sql}"
    );
    assert!(sql.contains("GROUP BY \"order\".\"buyer_id\""), "\n{sql}");
}

#[test]
fn aggregate_query_sqlite_casts_decimal_sum_to_text() {
    let sql = gen_for(AGG_SCHEMA, Dialect::Sqlite);
    // decimal sum → TEXT so it decodes as the wire string; int sum needs no cast.
    assert!(
        sql.contains("CAST(SUM(`order`.`total`) AS TEXT) AS `revenue`"),
        "\n{sql}"
    );
    assert!(sql.contains("SUM(`order`.`qty`) AS `units`"), "\n{sql}");
    assert!(
        sql.contains("CAST(AVG(`order`.`qty`) AS REAL) AS `avg_qty`"),
        "\n{sql}"
    );
}

const ENUM_HAVING_SCHEMA: &str = r#"
    enum Status { active, archived, paused }
    enum Level  { low = 1, mid = 2, high = 3 }
    @soft_delete(deleted_at)
    Order {
      id: Id
      deleted_at: timestamp?
      status: Status
      level: Level
      total: decimal(12, 2)
    }
    shape StatusStats from Order {
      st = status
      tier = level
      revenue = sum(total)
      n = count()
    }
    query stats() -> StatusStats[] {
      list Order
        group by (status, level)
        having (st = active and tier in (mid, high) and revenue > 100);
    }
"#;

#[test]
fn having_renders_enum_group_column_rhs_as_wire_literal() {
    // A `having` RHS that is a bare enum variant of a (possibly renamed) group column
    // lowers to the enum's wire value — a string literal for a string enum, the bare int
    // for an int enum — never a spurious column reference. Mirrors the `where` rendering.
    for dialect in [Dialect::MariaDb, Dialect::Postgres, Dialect::Sqlite] {
        let sql = gen_for(ENUM_HAVING_SCHEMA, dialect);
        let (o, s) = match dialect {
            Dialect::Postgres => ("\"order\"", "\""),
            _ => ("`order`", "`"),
        };
        // string enum → 'active'; int enum list → bare integers; never `order.active`.
        assert!(
            sql.contains(&format!("{o}.{s}status{s} = 'active'")),
            "{dialect:?}\n{sql}"
        );
        assert!(
            sql.contains(&format!("{o}.{s}level{s} IN (2, 3)")),
            "{dialect:?}\n{sql}"
        );
        assert!(
            !sql.contains("active\""),
            "{dialect:?} column-ref leak\n{sql}"
        );
        assert!(
            !sql.contains("active`"),
            "{dialect:?} column-ref leak\n{sql}"
        );
        // the aggregate comparison in the same HAVING is untouched.
        assert!(
            sql.contains(&format!("SUM({o}.{s}total{s}) > 100")),
            "{dialect:?}\n{sql}"
        );
    }
}
