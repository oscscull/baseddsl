use super::*;

// A keyset page whose primary sort key is a **nullable** column: the cursor comparison
// must be NULL-aware, or a plain `score < :keyset_0` drops every NULL-scored row from the
// walk (`NULL < v` is `NULL`, not true). Each dialect's own default NULL sort position —
// NULLs lowest on MariaDB/SQLite, highest on Postgres — drives the rendered predicate so
// it matches the (dialect-default) ORDER BY. The non-nullable `id` tiebreaker stays a
// plain comparison, and a fully non-nullable keyset (see `block_query_where_order_page…`)
// is unchanged.
#[test]
fn keyset_nullable_sort_key_is_null_aware_per_dialect() {
    let src = r#"
        @sort(id asc)
        Item { id: Id, name: text, score: int? }
        shape ItemCard from Item { name, score }
        query items() -> ItemCard[] { list Item order (score desc) page (2); }
        "#;

    // MariaDB/SQLite: NULLs sort lowest, so under `desc` they trail — a NULL cursor is the
    // last position (nothing after it) and non-NULL cursors also admit the trailing NULLs.
    let maria = gen(src);
    assert!(
        maria.contains(
            "(:keyset_0 IS NOT NULL AND (`item`.`score` < :keyset_0 OR `item`.`score` IS NULL))"
        ),
        "\n{maria}"
    );
    // Null-safe equality carries the tiebreaker chain across a NULL sort value.
    assert!(
        maria.contains("`item`.`score` <=> :keyset_0 AND `item`.`id` > :keyset_1"),
        "\n{maria}"
    );

    let lite = gen_for(src, Dialect::Sqlite);
    assert!(
        lite.contains(
            "(:keyset_0 IS NOT NULL AND (`item`.`score` < :keyset_0 OR `item`.`score` IS NULL))"
        ),
        "\n{lite}"
    );
    assert!(
        lite.contains("`item`.`score` IS :keyset_0 AND `item`.`id` > :keyset_1"),
        "\n{lite}"
    );

    // Postgres: NULLs sort highest, so under `desc` they lead — a NULL cursor is passed by
    // every non-NULL row, a non-NULL cursor excludes the already-seen NULL lead.
    let pg = gen_pg(src);
    assert!(
        pg.contains(
            "((:keyset_0 IS NULL AND \"item\".\"score\" IS NOT NULL) OR (:keyset_0 IS NOT NULL AND \"item\".\"score\" < :keyset_0))"
        ),
        "\n{pg}"
    );
    assert!(
        pg.contains(
            "\"item\".\"score\" IS NOT DISTINCT FROM :keyset_0 AND \"item\".\"id\" > :keyset_1"
        ),
        "\n{pg}"
    );
}

#[test]
fn null_comparison_lowers_to_is_null() {
    // `= null` / `!= null` are null tests, not value comparisons — SQL `col = NULL` never
    // matches. Covers the `where` body and a computed `case when` null-test.
    let sql = gen(r#"
        Order { id: Id  assignee: text?  @index(assignee) }
        shape Card from Order {
          id
          unassigned = case when assignee = null then true else false end
        }
        query only_unassigned() -> Card[] { list Order where (assignee = null) order (id); }
        query has_owner() -> Card[] { list Order where (assignee != null) order (id); }
        "#);
    assert!(
        sql.contains("`assignee` IS NULL"),
        "`where = null` must lower to IS NULL\n{sql}"
    );
    assert!(
        sql.contains("`assignee` IS NOT NULL"),
        "`where != null` must lower to IS NOT NULL\n{sql}"
    );
    assert!(
        sql.contains("`assignee` IS NULL THEN"),
        "a computed `case when col = null` must null-test (`… IS NULL THEN …`), not `= NULL`\n{sql}"
    );
}

#[test]
fn nulls_placement_lowers_per_dialect() {
    let src = r#"
        Order { id: Id  assignee: text?  @index(assignee) }
        shape Card from Order { id }
        query q_last() -> Card[] { list Order order (assignee asc nulls last); }
        query q_first() -> Card[] { list Order order (assignee asc nulls first); }
    "#;
    // MariaDB/MySQL have no NULLS clause → a leading `col IS NULL` term (ASC trails NULLs,
    // `DESC` leads them).
    let my = gen(src);
    assert!(
        my.contains("`assignee` IS NULL, `order`.`assignee` ASC"),
        "nulls last (MariaDB)\n{my}"
    );
    assert!(
        my.contains("`assignee` IS NULL DESC, `order`.`assignee` ASC"),
        "nulls first (MariaDB)\n{my}"
    );
    // Postgres: native NULLS LAST / NULLS FIRST.
    let pg = gen_pg(src);
    assert!(
        pg.contains("\"assignee\" ASC NULLS LAST"),
        "nulls last (Postgres)\n{pg}"
    );
    assert!(
        pg.contains("\"assignee\" ASC NULLS FIRST"),
        "nulls first (Postgres)\n{pg}"
    );
}

#[test]
fn nulls_placement_honored_in_a_to_many_nest() {
    // A to-many nest orders by the child model's `@sort`, which may carry `nulls last`; the
    // nest's ORDER BY (inside the JSON aggregate) must honor it, not just the top-level order.
    let src = r#"
        Order { id: Id  items: Item[] (Item.order) }
        @sort(note asc nulls last)
        Item { id: Id  order: Order  note: text?  @index(note) }
        shape OrderDetail from Order { id  items { id } }
        query q() -> OrderDetail[];
    "#;
    let my = gen(src);
    assert!(
        my.contains("`note` IS NULL, "),
        "nest nulls last (MariaDB)\n{my}"
    );
    let pg = gen_pg(src);
    assert!(
        pg.contains("\"note\" ASC NULLS LAST"),
        "nest nulls last (Postgres)\n{pg}"
    );
}
