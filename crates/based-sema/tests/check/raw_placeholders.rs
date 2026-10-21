use super::*;

// ---------- raw `{table}` / `{id}` placeholders -------------------------------

const TASK: &str = r#"
    Task { id: Id, title: text }
    Ask { id: Id, task: Task, answer: text? }
    shape T from Task { title }
"#;

fn count(d: &[Diagnostic], code: &str) -> usize {
    d.iter().filter(|x| x.code == code).count()
}

#[test]
fn bare_id_in_correlated_subquery_is_clean() {
    assert_clean(&format!(
        "{TASK}\nquery q() -> T[] {{ list Task where (raw`EXISTS (SELECT 1 FROM ask a WHERE a.task_id = {{id}} AND a.answer IS NULL)`) order (title); }}"
    ));
}

#[test]
fn table_dot_id_in_predicate_is_rejected() {
    let (_, d) = analyze(&format!(
        "{TASK}\nquery q() -> T[] {{ list Task where (raw`EXISTS (SELECT 1 FROM ask a WHERE a.task_id = {{table}}.{{id}})`) order (title); }}"
    ));
    assert!(errors(&d).contains(&"E0353"), "{:?}", codes(&d));
    let note = d
        .iter()
        .find(|x| x.code == "E0353")
        .map(|x| format!("{x:?}"));
    assert!(note.is_some_and(|n| n.contains("task.id")), "{d:?}");
}

#[test]
fn alias_dot_id_is_rejected() {
    let (_, d) = analyze(&format!(
        "{TASK}\nquery q() -> T[] {{ list Task where (raw`t . {{id}} IS NOT NULL`) order (title); }}"
    ));
    assert!(errors(&d).contains(&"E0353"), "{:?}", codes(&d));
}

#[test]
fn table_dot_id_in_named_filter_is_reported_once() {
    let (_, d) = analyze(&format!(
        "{TASK}\nfilter open_ask = raw`EXISTS (SELECT 1 FROM ask a WHERE a.task_id = {{table}}.{{id}})`;\nquery q() -> T[] {{ list Task where (open_ask) order (title); }}"
    ));
    assert_eq!(count(&d, "E0353"), 1, "{:?}", codes(&d));
}

#[test]
fn table_dot_id_in_shape_value_is_rejected() {
    let (_, d) = analyze(
        r#"
        Task { id: Id, title: text }
        shape T from Task { title, n = raw`(SELECT 1 FROM ask a WHERE a.task_id = {table}.{id})` }
        query q() -> T[] { list Task order (title); }
        "#,
    );
    assert!(errors(&d).contains(&"E0353"), "{:?}", codes(&d));
}

#[test]
fn table_dot_id_in_whole_query_body_is_rejected() {
    let (_, d) = analyze(
        r#"
        Task { id: Id, title: text }
        shape T from Task { title }
        query q() -> T[] { raw`SELECT title FROM {table} WHERE {table}.{id} IS NOT NULL`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0353"), "{:?}", codes(&d));
}

#[test]
fn placeholder_in_raw_write_is_rejected() {
    let (_, d) = analyze(
        r#"
        Task { id: Id, title: text }
        mutation touch() -> Task { raw`UPDATE {table} SET title = title`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0354"), "{:?}", codes(&d));
}

#[test]
fn raw_write_without_placeholders_passes_the_placeholder_check() {
    let (_, d) = analyze(
        r#"
        Task { id: Id, title: text }
        mutation touch() -> Task { raw`UPDATE task SET title = title`; }
        "#,
    );
    assert!(!codes(&d).contains(&"E0354"), "{:?}", codes(&d));
}
