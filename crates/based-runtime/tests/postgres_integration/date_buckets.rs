use super::*;

/// `date(x)` / `week(x)` bucket `timestamptz` values by their UTC day and ISO week, live on
/// Postgres: a Sunday belongs to the week of the Monday before it, a Monday starts its own
/// week, and a value written with a non-UTC offset buckets by its UTC instant.
#[tokio::test]
async fn date_and_week_buckets_live_postgres() {
    const SCHEMA: &str = r#"
        Signup { id: text, created_at: timestamp }
        shape PerDay from Signup { day = date(created_at), n = count() }
        shape PerWeek from Signup { week = week(created_at), n = count() }
        shape Joined from Signup { id, day = date(created_at), week = week(created_at) }
        query per_day() -> PerDay[] { list Signup group by (date(created_at)) order (day); }
        query per_week() -> PerWeek[] { list Signup group by (week(created_at)) order (week); }
        query joined() -> Joined[] { list Signup order (id); }
    "#;
    let Some((c, router, container)) = live_schema(SCHEMA).await else {
        return;
    };
    // s1 Tuesday, s2 Sunday 23:30 UTC (written at +02:00), s3/s4 Monday, s5 Sunday.
    container
        .exec_batch(
            "INSERT INTO \"signup\" (\"id\", \"created_at\") VALUES \
             ('s1', '2026-11-03 08:00:00+00'), \
             ('s2', '2026-11-09 01:30:00+02'), \
             ('s3', '2026-11-09 00:15:00+00'), \
             ('s4', '2026-11-09 22:00:00+00'), \
             ('s5', '2026-11-15 12:00:00+00');",
        )
        .await;

    let per_day = call(&c, &router, "POST", "/q/per_day", json!({}), json!({})).await;
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

    let per_week = call(&c, &router, "POST", "/q/per_week", json!({}), json!({})).await;
    assert_eq!(per_week.status, 200, "{:?}", per_week.body);
    assert_eq!(
        per_week.body,
        json!([
            { "week": "2026-11-02", "n": 2 },
            { "week": "2026-11-09", "n": 3 }
        ])
    );

    let joined = call(&c, &router, "POST", "/q/joined", json!({}), json!({})).await;
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
