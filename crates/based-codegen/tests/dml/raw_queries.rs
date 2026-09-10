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
