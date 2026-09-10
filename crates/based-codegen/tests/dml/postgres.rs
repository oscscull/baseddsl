use super::*;

// ---------- Postgres  -------------------------------------------------

#[test]
fn pg_select_double_quotes_identifiers_and_keeps_named_placeholders() {
    // Postgres double-quotes idents; the emitted template still carries `:name`
    // placeholders (the runtime rewrites them to `$n`), and the injected tombstone
    // uses the same `IS NULL` predicate.
    let sql = gen_pg(
        r#"
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp?, status: text, total: int }
        shape OrderCard from Order { status, total }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(sql.contains("FROM \"order\""), "\n{sql}");
    assert!(
        sql.contains("\"order\".\"status\" AS \"status\""),
        "\n{sql}"
    );
    assert!(
        sql.contains("WHERE \"order\".\"id\" = :id AND \"order\".\"deleted_at\" IS NULL"),
        "\n{sql}"
    );
    // no backtick-quoted identifiers in the statement body (the header has backticks).
    let body = &sql[sql.find("SELECT").unwrap()..];
    assert!(!body.contains('`'), "\n{sql}");
}

#[test]
fn pg_bare_bool_uses_true_keyword() {
    let sql = gen_pg(
        r#"
        @sort(id asc)
        Order { id: Id, active: bool, total: int }
        shape O from Order { total }
        query live() -> O[] { list Order where (active); }
        "#,
    );
    assert!(sql.contains("\"order\".\"active\" = TRUE"), "\n{sql}");
}

#[test]
fn pg_has_uses_jsonb_containment_operator() {
    // `has` is JSON-array containment: Postgres's `arr @> value`, not MySQL's
    // `value MEMBER OF(arr)`.
    let sql = gen_pg(
        r#"
        @sort(id asc)
        Order { id: Id, tags: text[], total: int }
        shape O from Order { total }
        query tagged(tag: text) -> O[] { list Order where (tags has $tag); }
        "#,
    );
    assert!(sql.contains("\"order\".\"tags\" @> :tag"), "\n{sql}");
    assert!(!sql.contains("MEMBER OF"), "\n{sql}");
}

#[test]
fn pg_join_double_quotes_alias_and_on() {
    let sql = gen_pg(
        r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        Order { id: Id, placed_by: User, total: int }
        shape OrderCard from Order { who = placed_by.name }
        query order_by_id(id) -> OrderCard;
        "#,
    );
    assert!(
        sql.contains(
            "JOIN \"user\" AS \"j_placed_by\" ON \"j_placed_by\".\"id\" = \"order\".\"placed_by_id\" AND \"j_placed_by\".\"deleted_at\" IS NULL"
        ),
        "\n{sql}"
    );
}
