use super::*;

// ---------- decorators -----------------------------------------------------

#[test]
fn soft_delete_bad_type() {
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Doc { id: Id, deleted_at: int }
        "#,
    );
    assert_eq!(errors(&d), ["E0120"]);
}

#[test]
fn soft_delete_requires_nullable_timestamp() {
    // non-nullable timestamp is not the covered subset
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Doc { id: Id, deleted_at: timestamp }
        "#,
    );
    assert_eq!(errors(&d), ["E0120"]);
}

#[test]
fn unknown_decorator_warns() {
    let (_, d) = analyze("@wat\nDoc { id: Id, name: text }");
    assert_eq!(codes(&d), ["W0101"]);
}

#[test]
fn index_unknown_column() {
    let (_, d) = analyze("Doc { id: Id, name: text\n @index nope }");
    assert_eq!(errors(&d), ["E0122"]);
}
