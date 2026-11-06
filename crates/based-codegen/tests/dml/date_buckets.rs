use super::*;

// ---------- date buckets: `date(x)` / `week(x)` ------------------------------

const BUCKET_SCHEMA: &str = r#"
    Signup { id: Id, created_at: timestamp, born_on: date, plan: text }
    shape PerDay from Signup { day = date(created_at), n = count() }
    shape PerWeek from Signup { week = week(created_at), n = count() }
    shape PerBirthWeek from Signup { week = week(born_on), n = count() }
    shape Row from Signup { id, day = date(created_at) }
    query per_day() -> PerDay[] { list Signup group by (date(created_at)) order (day); }
    query per_week() -> PerWeek[] { list Signup group by (week(created_at)) order (week); }
    query per_birth_week() -> PerBirthWeek[] { list Signup group by (week(born_on)); }
    query rows() -> Row[] { list Signup; }
"#;

#[test]
fn date_buckets_lower_on_sqlite() {
    let sql = gen_for(BUCKET_SCHEMA, Dialect::Sqlite);
    let day = query_section(&sql, "per_day");
    assert!(
        day.contains("date(`signup`.`created_at`) AS `day`"),
        "\n{day}"
    );
    assert!(
        day.contains("GROUP BY date(`signup`.`created_at`)"),
        "\n{day}"
    );
    assert!(
        day.contains("ORDER BY date(`signup`.`created_at`)"),
        "\n{day}"
    );
    let week = query_section(&sql, "per_week");
    assert!(
        week.contains("date(`signup`.`created_at`, 'weekday 0', '-6 days') AS `week`"),
        "\n{week}"
    );
    assert!(
        week.contains("GROUP BY date(`signup`.`created_at`, 'weekday 0', '-6 days')"),
        "\n{week}"
    );
    let rows = query_section(&sql, "rows");
    assert!(
        rows.contains("date(`signup`.`created_at`) AS `day`"),
        "\n{rows}"
    );
}

#[test]
fn date_buckets_lower_on_mariadb() {
    let sql = gen(BUCKET_SCHEMA);
    let day = query_section(&sql, "per_day");
    assert!(
        day.contains("DATE(`signup`.`created_at`) AS `day`"),
        "\n{day}"
    );
    assert!(
        day.contains("GROUP BY DATE(`signup`.`created_at`)"),
        "\n{day}"
    );
    let week = query_section(&sql, "per_week");
    let monday = "(DATE(`signup`.`created_at`) - INTERVAL WEEKDAY(`signup`.`created_at`) DAY)";
    assert!(week.contains(&format!("{monday} AS `week`")), "\n{week}");
    assert!(week.contains(&format!("GROUP BY {monday}")), "\n{week}");
}

#[test]
fn date_buckets_lower_on_postgres_at_utc() {
    let sql = gen_pg(BUCKET_SCHEMA);
    let day = query_section(&sql, "per_day");
    let utc_day = "((\"signup\".\"created_at\" AT TIME ZONE 'UTC'))::date";
    assert!(day.contains(&format!("{utc_day} AS \"day\"")), "\n{day}");
    assert!(day.contains(&format!("GROUP BY {utc_day}")), "\n{day}");
    let week = query_section(&sql, "per_week");
    let utc_week =
        "date_trunc('week', ((\"signup\".\"created_at\" AT TIME ZONE 'UTC'))::timestamp)::date";
    assert!(
        week.contains(&format!("{utc_week} AS \"week\"")),
        "\n{week}"
    );
    assert!(week.contains(&format!("GROUP BY {utc_week}")), "\n{week}");
    // A `date` column has no zone to normalize.
    let birth = query_section(&sql, "per_birth_week");
    assert!(
        birth.contains("date_trunc('week', (\"signup\".\"born_on\")::timestamp)::date"),
        "\n{birth}"
    );
}
