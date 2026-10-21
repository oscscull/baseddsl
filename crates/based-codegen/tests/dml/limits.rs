use super::*;

// `limit ($p)` binds the param into `LIMIT`; a literal inlines; plain rows, no keyset.
#[test]
fn param_limit_binds_and_literal_inlines() {
    let src = r#"
        @sort(id asc)
        Run { id: Id, schedule_id: text, started_at: timestamp }
        query recent(sid: text, n: int) -> Run[] {
          list Run where (schedule_id = $sid) order (started_at desc) limit ($n, max 50);
        }
        query top() -> Run[] { list Run limit (3); }
        "#;
    for out in [gen(src), gen_pg(src), gen_for(src, Dialect::Sqlite)] {
        let recent = query_section(&out, "recent");
        assert!(recent.contains("DESC\nLIMIT :n;"), "\n{recent}");
        assert!(!recent.contains("__keyset"), "\n{recent}");
        let top = query_section(&out, "top");
        assert!(top.contains("\nLIMIT 3;"), "\n{top}");
    }
}

#[test]
fn aggregate_query_takes_a_limit() {
    let out = gen(r#"
        City { id: Id, region: text }
        shape RegionCount from City { region, n = count() }
        query top(k: int) -> RegionCount[] {
          list City group by (region) order (n desc) limit ($k);
        }
        "#);
    let top = query_section(&out, "top");
    assert!(top.contains("DESC\nLIMIT :k;"), "\n{top}");
}
