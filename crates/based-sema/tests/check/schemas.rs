use super::*;

// ---------- @schema / database namespace qualifier ------------------------

#[test]
fn schema_qualifier_is_accepted_and_recorded() {
    // A valid `@schema("name")` places the model in a named namespace — no diagnostic.
    let (schema, d) = analyze(
        r#"
        @schema("analytics")
        Event { id: Id, name: text }
        "#,
    );
    assert_eq!(errors(&d), Vec::<&str>::new());
    assert_eq!(
        schema.model("Event").unwrap().schema.as_deref(),
        Some("analytics")
    );
}

#[test]
fn schema_qualifier_rejects_an_invalid_name() {
    // Empty, dotted (multi-level), or whitespace-carrying names are E0296.
    for bad in [
        r#"@schema("")"#,
        r#"@schema("a.b")"#,
        r#"@schema("has space")"#,
    ] {
        let (_, d) = analyze(&format!("{bad} Event {{ id: Id, name: text }}"));
        assert_eq!(errors(&d), ["E0296"], "for {bad}");
    }
}

#[test]
fn dotted_table_name_points_at_schema() {
    // A `.` in `@table` is a misplaced namespace prefix — E0297 steers to `@schema`.
    let (_, d) = analyze(
        r#"
        @table("analytics.events")
        Event { id: Id, name: text }
        "#,
    );
    assert_eq!(errors(&d), ["E0297"]);
}
