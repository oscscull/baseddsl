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

// ---------- `{id}` against a composite key -----------------------------------

const READING: &str = r#"
    Device { id: Id, name: text }
    @key(device, seq)
    Reading { device: Device, seq: int, value: int }
    shape R from Reading { value }
"#;

#[test]
fn id_on_composite_key_predicate_is_rejected() {
    let (_, d) = analyze(&format!(
        "{READING}\nquery q() -> R[] {{ list Reading where (raw`{{id}} IS NOT NULL`) order (value); }}"
    ));
    assert_eq!(count(&d, "E0356"), 1, "{:?}", codes(&d));
    let msg = d
        .iter()
        .find(|x| x.code == "E0356")
        .map(|x| format!("{x:?}"));
    assert!(
        msg.is_some_and(|m| m.contains("device_id") && m.contains("reading.seq")),
        "{d:?}"
    );
}

#[test]
fn id_on_composite_key_named_filter_is_rejected_at_the_call() {
    let (_, d) = analyze(&format!(
        "{READING}\nfilter keyed = raw`{{id}} > 0`;\nquery q() -> R[] {{ list Reading where (keyed) order (value); }}"
    ));
    assert_eq!(count(&d, "E0356"), 1, "{:?}", codes(&d));
}

#[test]
fn id_on_composite_key_shape_value_is_rejected() {
    let (_, d) = analyze(
        r#"
        Device { id: Id, name: text }
        @key(device, seq)
        Reading { device: Device, seq: int, value: int }
        shape R from Reading { value, k = raw`{id}` }
        query q() -> R[] { list Reading order (value); }
        "#,
    );
    assert!(errors(&d).contains(&"E0356"), "{:?}", codes(&d));
}

#[test]
fn id_on_composite_key_having_is_rejected() {
    let (_, d) = analyze(
        r#"
        Device { id: Id, name: text }
        @key(device, seq)
        Reading { device: Device, seq: int, value: int }
        shape S from Reading { v = value, n = count() }
        query q() -> S[] { list Reading group by (value) having (raw`count({id}) > 1`); }
        "#,
    );
    assert!(errors(&d).contains(&"E0356"), "{:?}", codes(&d));
}

#[test]
fn id_on_composite_key_whole_query_is_rejected() {
    let (_, d) = analyze(&format!(
        "{READING}\nquery q() -> R[] {{ raw`SELECT value FROM {{table}} WHERE {{id}} > 0`; }}"
    ));
    assert!(errors(&d).contains(&"E0356"), "{:?}", codes(&d));
}

#[test]
fn id_on_composite_key_join_is_rejected() {
    let (_, d) = analyze(
        r#"
        Device { id: Id, name: text, legacy: int }
        @key(dev_ref, seq)
        Reading {
          dev_ref: int
          seq: int
          device: Device (on: reading.dev_ref = device.legacy and raw`{id} IS NOT NULL`)
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0356"), "{:?}", codes(&d));
}

#[test]
fn id_on_single_key_is_clean() {
    assert_clean(
        r#"
        @key(sku)
        Product { sku: text, name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (raw`{id} <> ''`) order (name); }
        "#,
    );
}
