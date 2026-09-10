use super::*;

// ---------- @no_id keyless legacy tables ----------------------------------

#[test]
fn no_id_suppresses_the_missing_id_error() {
    // A keyless legacy table opts out of the primary key with a reason; no E0261.
    assert_clean(
        r#"
        @no_id("legacy audit log has no surrogate key")
        Event { source: text (unique), payload: text }
        shape E from Event { source, payload }
        query event_by_source(source) -> E;
        "#,
    );
}

#[test]
fn no_id_requires_a_non_empty_reason() {
    let (_, d) = analyze(
        r#"
        @no_id
        Event { source: text (unique) }
        "#,
    );
    assert_eq!(errors(&d), ["E0262"]);
    let (_, d) = analyze(
        r#"
        @no_id("")
        Event { source: text (unique) }
        "#,
    );
    assert_eq!(errors(&d), ["E0262"]);
}
