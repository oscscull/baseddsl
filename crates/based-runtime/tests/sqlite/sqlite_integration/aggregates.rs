use super::*;

/// A real `GROUP BY` / `HAVING` aggregate query executes against SQLite: `count()`, `sum`
/// (int + decimal), `avg`, and `max` group per buyer, soft-deleted rows are excluded before
/// grouping, `having` filters groups, and `order` sorts them — all computed in the database
/// and decoded to the declared wire types. `sum`/`max` over a `decimal` is float-degraded
/// on SQLite (documented — decimal is TEXT affinity there; production dialects DECIMAL/
/// NUMERIC are exact), so the exact-value proofs use int columns.
#[tokio::test]
async fn aggregate_group_by_having_end_to_end() {
    let c = compile_sqlite(
        r#"
        Buyer { id: Id, name: text }
        @soft_delete(deleted_at)
        Order {
          id: Id
          deleted_at: timestamp?
          buyer:      Buyer
          total:      decimal(12, 2)
          qty:        int
        }
        shape BuyerStats from Order {
          buyer   = buyer
          orders  = count()
          revenue = sum(total)
          units   = sum(qty)
          avg_qty = avg(qty)
          top_qty = max(qty)
        }
        query buyer_stats() -> BuyerStats[] {
          list Order group by (buyer) having (units > 3) order (units desc);
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            "INSERT INTO `buyer` (`id`, `name`) VALUES ('b1', 'Ada'), ('b2', 'Bo');
             INSERT INTO `order` (`id`, `buyer_id`, `total`, `qty`, `deleted_at`) VALUES
               ('o1', 'b1', '100.00',  2, NULL),
               ('o2', 'b1',  '50.50',  3, NULL),
               ('o3', 'b1',  '99.99',  9, '2024-01-01 00:00:00'),
               ('o4', 'b2',  '10.00',  4, NULL);",
        )
        .await
        .expect("seed");

    let got = call(&c, &backend, "POST", "/q/buyer_stats", json!({}), json!({})).await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    // Only Ada's two live orders count (o3 is soft-deleted, so its qty 9 is excluded
    // *before* grouping). units = 2 + 3 = 5 for b1, 4 for b2 (both > 3, pass `having`);
    // `order (units desc)` puts b1 first. count = 2/1, avg_qty = 2.5/4.0, max qty = 3/4
    // (exact on int). revenue is the SQLite float-degraded decimal sum.
    assert_eq!(
        got.body,
        json!([
            {
                "buyer": "b1",
                "orders": 2,
                "revenue": "150.5",
                "units": 5,
                "avg_qty": 2.5,
                "top_qty": 3
            },
            {
                "buyer": "b2",
                "orders": 1,
                "revenue": "10.0",
                "units": 4,
                "avg_qty": 4.0,
                "top_qty": 4
            }
        ])
    );
}

#[tokio::test]
async fn aggregate_having_on_enum_group_column_end_to_end() {
    // A `having` predicate that filters a grouped *enum* column by a bare variant must
    // lower the RHS to the enum's wire value (a string literal, or a bare int) — not a
    // spurious column reference (which is invalid SQL that never executes). Exercises both
    // enum kinds and the renamed-group-column path.
    let c = compile_sqlite(
        r#"
        enum Status { active, archived, paused }
        enum Level  { low = 1, mid = 2, high = 3 }
        Order {
          id:     Id
          status: Status
          level:  Level
          total:  decimal(12, 2)
        }
        shape StatusStats from Order {
          st      = status
          tier    = level
          revenue = sum(total)
          n       = count()
        }
        query stats() -> StatusStats[] {
          list Order
            group by (status, level)
            having (st in (active, paused) and tier = high and revenue > 100)
            order (revenue desc);
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            "INSERT INTO `order` (`id`, `status`, `level`, `total`) VALUES
               ('o1', 'active',   3, '100.00'),
               ('o2', 'active',   3,  '60.00'),
               ('o3', 'paused',   3,  '40.00'),
               ('o4', 'archived', 3, '500.00'),
               ('o5', 'active',   2, '999.00');",
        )
        .await
        .expect("seed");

    let got = call(&c, &backend, "POST", "/q/stats", json!({}), json!({})).await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    // Groups are (status, level). `having` keeps only groups whose status is active/paused
    // (excludes 'archived'), level is high=3 (excludes the level=2 active group), and
    // revenue > 100. (active,3): revenue 160 ✓. (paused,3): revenue 40 ✗. (archived,3):
    // status not in (active,paused) ✗. (active,2): level ≠ high ✗. Only (active,3) survives.
    assert_eq!(
        got.body,
        json!([
            { "st": "active", "tier": 3, "revenue": "160.0", "n": 2 }
        ])
    );
}

/// `date(x)` / `week(x)` bucket stored UTC timestamps by day and by ISO week: a Sunday
/// belongs to the week of the Monday before it, a Monday starts its own week.
#[tokio::test]
async fn date_and_week_buckets_end_to_end() {
    let c = compile_sqlite(
        r#"
        Signup { id: text, created_at: timestamp }
        shape PerDay from Signup { day = date(created_at), n = count() }
        shape PerWeek from Signup { week = week(created_at), n = count() }
        shape Joined from Signup { id, day = date(created_at), week = week(created_at) }
        query per_day() -> PerDay[] { list Signup group by (date(created_at)) order (day); }
        query per_week() -> PerWeek[] { list Signup group by (week(created_at)) order (week); }
        query joined() -> Joined[] { list Signup order (id); }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    // s1 Tuesday, s2 Sunday late, s3/s4 Monday, s5 Sunday.
    backend
        .execute_batch(
            "INSERT INTO `signup` (`id`, `created_at`) VALUES
               ('s1', '2026-11-03T08:00:00.000000Z'),
               ('s2', '2026-11-08T23:30:00.000000Z'),
               ('s3', '2026-11-09T00:15:00.000000Z'),
               ('s4', '2026-11-09T22:00:00.000000Z'),
               ('s5', '2026-11-15T12:00:00.000000Z');",
        )
        .await
        .expect("seed");

    let per_day = call(&c, &backend, "POST", "/q/per_day", json!({}), json!({})).await;
    assert_eq!(per_day.status, 200, "{:?}", per_day.body);
    assert_eq!(
        per_day.body,
        json!([
            { "day": "2026-11-03", "n": 1 },
            { "day": "2026-11-08", "n": 1 },
            { "day": "2026-11-09", "n": 2 },
            { "day": "2026-11-15", "n": 1 }
        ])
    );

    let per_week = call(&c, &backend, "POST", "/q/per_week", json!({}), json!({})).await;
    assert_eq!(per_week.status, 200, "{:?}", per_week.body);
    assert_eq!(
        per_week.body,
        json!([
            { "week": "2026-11-02", "n": 2 },
            { "week": "2026-11-09", "n": 3 }
        ])
    );

    let joined = call(&c, &backend, "POST", "/q/joined", json!({}), json!({})).await;
    assert_eq!(joined.status, 200, "{:?}", joined.body);
    assert_eq!(
        joined.body,
        json!([
            { "id": "s1", "day": "2026-11-03", "week": "2026-11-02" },
            { "id": "s2", "day": "2026-11-08", "week": "2026-11-02" },
            { "id": "s3", "day": "2026-11-09", "week": "2026-11-09" },
            { "id": "s4", "day": "2026-11-09", "week": "2026-11-09" },
            { "id": "s5", "day": "2026-11-15", "week": "2026-11-09" }
        ])
    );
}
