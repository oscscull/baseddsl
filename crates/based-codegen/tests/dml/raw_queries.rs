use super::*;

#[test]
fn raw_query_body_is_the_statement_with_bound_params() {
    // A whole-query raw body lowers verbatim: `${param}` → `:param`, `{table}` →
    // the target's quoted table, and NOTHING engine-built is injected — no
    // soft-delete tombstone, no ORDER BY, no LIMIT.
    let src = r#"
        @soft_delete(deleted_at)
        @sort(name asc)
        User { id: Id, deleted_at: timestamp?, name: text, total: int }
        shape UserRow from User { name }
        query heavy(min: int) -> UserRow[] {
          raw`SELECT u.name AS name FROM {table} u WHERE u.total >= ${min}`;
        }
        "#;
    for (dialect, table) in [
        (Dialect::MariaDb, "`user`"),
        (Dialect::Sqlite, "`user`"),
        (Dialect::Postgres, "\"user\""),
    ] {
        let out = gen_for(src, dialect);
        assert!(
            out.contains(&format!(
                "SELECT u.name AS name FROM {table} u WHERE u.total >= :min;"
            )),
            "\n{out}"
        );
        assert!(!out.contains("deleted_at"), "\n{out}");
        assert!(!out.contains("ORDER BY"), "\n{out}");
        assert!(!out.contains("LIMIT"), "\n{out}");
    }
}

#[test]
fn raw_query_trailing_semicolon_is_normalized() {
    // A raw body already ending in `;` emits exactly one terminator.
    let out = gen(r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query all() -> UserRow[] { raw`SELECT name FROM user;`; }
        "#);
    assert!(out.contains("SELECT name FROM user;\n"), "\n{out}");
    assert!(!out.contains(";;"), "\n{out}");
}

/// Each dialect paired with how it spells the schema-qualified `core.user` table.
const CORE_USER: [(Dialect, &str); 4] = [
    (Dialect::MariaDb, "`core`.`user`"),
    (Dialect::MySql, "`core`.`user`"),
    (Dialect::Sqlite, "`core`.`user`"),
    (Dialect::Postgres, "\"core\".\"user\""),
];

#[test]
fn raw_query_table_carries_the_model_schema() {
    // `{table}` in a whole-query body names the same schema-qualified table as the
    // rest of codegen, so an aliased `FROM {table} u` reads the right namespace.
    let src = r#"
        @schema("core")
        User { id: Id, name: text, total: int }
        shape UserRow from User { name }
        query heavy(min: int) -> UserRow[] {
          raw`SELECT u.name AS name FROM {table} u WHERE u.total >= ${min}`;
        }
        "#;
    for (dialect, table) in CORE_USER {
        let out = gen_for(src, dialect);
        assert!(
            out.contains(&format!("FROM {table} u WHERE u.total >= :min;")),
            "{dialect:?}\n{out}"
        );
    }
}

#[test]
fn raw_leaf_table_carries_the_model_schema() {
    // `{table}` in a shape field and a `where` leaf is schema-qualified too.
    let src = r#"
        @schema("core")
        User { id: Id, name: text, total: int }
        shape UserRow from User { name, n = raw`(SELECT count(*) FROM {table})` }
        query rich() -> UserRow[] {
          list User where (raw`EXISTS (SELECT 1 FROM {table} x WHERE x.total > 0)`) order (name);
        }
        "#;
    for (dialect, table) in CORE_USER {
        let out = gen_for(src, dialect);
        assert!(
            out.contains(&format!("(SELECT count(*) FROM {table})")),
            "{dialect:?}\n{out}"
        );
        assert!(
            out.contains(&format!(
                "EXISTS (SELECT 1 FROM {table} x WHERE x.total > 0)"
            )),
            "{dialect:?}\n{out}"
        );
    }
}
