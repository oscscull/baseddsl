use super::*;

// ---------- `list distinct` -----------------------------------------------

#[test]
fn distinct_list_emits_select_distinct_and_suppresses_model_sort() {
    // `list distinct` dedups the projected rows. The model `@sort` default is NOT
    // applied (a non-projected sort column would break `SELECT DISTINCT`), so with no
    // explicit `order` there is no `ORDER BY` at all.
    let ddl = gen(r#"
        @sort(name asc)
        City { id: Id, name: text, region: text }
        shape RegionName from City { region }
        query regions() -> RegionName[] { list distinct City; }
        "#);
    assert!(ddl.contains("SELECT DISTINCT"), "\n{ddl}");
    assert!(ddl.contains("`city`.`region` AS `region`"), "\n{ddl}");
    // model `@sort(name asc)` is suppressed — no ORDER BY, and `name` is never selected.
    assert!(!ddl.contains("ORDER BY"), "\n{ddl}");
    assert!(!ddl.contains("`name`"), "\n{ddl}");
}

#[test]
fn distinct_list_keeps_explicit_order_over_projected_column() {
    // An explicit `order` on a projected column survives; still `SELECT DISTINCT`.
    let ddl = gen(r#"
        City { id: Id, name: text, region: text }
        shape RegionName from City { region }
        query regions() -> RegionName[] { list distinct City order (region desc); }
        "#);
    assert!(ddl.contains("SELECT DISTINCT"), "\n{ddl}");
    assert!(ddl.contains("ORDER BY `city`.`region` DESC"), "\n{ddl}");
}

#[test]
fn distinct_list_with_offset_page_has_no_keyset_tiebreaker() {
    // Offset pagination composes with distinct; the id keyset tiebreaker is NOT appended
    // (it would defeat the dedup and is only for keyset pages anyway).
    let ddl = gen_pg(
        r#"
        City { id: Id, name: text, region: text }
        shape RegionName from City { region }
        query regions() -> RegionName[] { list distinct City order (region desc) page (20) offset; }
        "#,
    );
    assert!(ddl.contains("SELECT DISTINCT"), "\n{ddl}");
    assert!(ddl.contains("LIMIT 20 OFFSET :offset"), "\n{ddl}");
    // no `id` appended to ORDER BY.
    assert!(!ddl.contains("\"id\""), "\n{ddl}");
}

#[test]
fn distinct_with_count_counts_deduped_rows_via_derived_table() {
    // `with count` under `distinct` must total the *deduped* rows: the count wraps the
    // `SELECT DISTINCT <projection>` in a derived table rather than `COUNT(*)`-ing the
    // raw joined rows (which would overcount every duplicated projection).
    let ddl = gen_pg(
        r#"
        City { id: Id, name: text, region: text }
        shape RegionName from City { region }
        query regions() -> RegionName[] {
          list distinct City order (region desc) page (20) offset with count;
        }
        "#,
    );
    assert!(
        ddl.contains("FROM (SELECT DISTINCT"),
        "distinct count must wrap the deduped projection\n{ddl}"
    );
    assert!(
        ddl.contains(r#"AS "distinct_rows""#),
        "derived table needs an alias\n{ddl}"
    );
    // A bare `COUNT(*) FROM "city"` (undeduped) must NOT be the count query.
    assert!(
        !ddl.contains("SELECT COUNT(*) AS \"count\"\nFROM \"city\""),
        "distinct count must not count raw rows\n{ddl}"
    );
}
