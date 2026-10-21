use super::*;

// ---------- soft-delete `read|delete|restore: raw` overrides (E0357) ------------

fn order(members: &str) -> String {
    format!(
        "@soft_delete(deleted_at)\nOrder {{ id: Id, deleted_at: timestamp?, archived: bool\n{members}\n}}\nshape O from Order {{ id }}\nquery q() -> O[] {{ list Order; }}"
    )
}

#[test]
fn overrides_are_recorded_on_the_soft_delete() {
    let (s, d) = analyze(&order(
        "read: raw`deleted_at IS NULL AND NOT archived`\ndelete: raw`archived = true`\nrestore: raw`archived = false`",
    ));
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let sd = s.model("Order").unwrap().soft_delete.as_ref().unwrap();
    assert!(sd.overrides.read.is_some());
    assert!(sd.overrides.delete.is_some());
    assert!(sd.overrides.restore.is_some());
}

#[test]
fn override_without_soft_delete_is_rejected() {
    let (_, d) = analyze(
        "Order { id: Id, archived: bool\nread: raw`NOT archived`\n}\nshape O from Order { id }\nquery q() -> O[] { list Order; }",
    );
    assert!(errors(&d).contains(&"E0357"), "{:?}", codes(&d));
}

#[test]
fn repeated_op_is_rejected() {
    let (_, d) = analyze(&order(
        "delete: raw`archived = true`\ndelete: raw`archived = false`",
    ));
    assert!(errors(&d).contains(&"E0357"), "{:?}", codes(&d));
}

#[test]
fn param_in_override_is_rejected() {
    let (_, d) = analyze(&order("read: raw`archived = ${flag}`"));
    assert!(errors(&d).contains(&"E0357"), "{:?}", codes(&d));
}

#[test]
fn whole_statement_is_rejected() {
    let (_, d) = analyze(&order(
        "restore: raw`update {table} set deleted_at = null where id = {id}`",
    ));
    assert!(errors(&d).contains(&"E0357"), "{:?}", codes(&d));
    let (_, d) = analyze(&order("read: raw`WHERE deleted_at IS NULL`"));
    assert!(errors(&d).contains(&"E0357"), "{:?}", codes(&d));
}

#[test]
fn empty_override_is_rejected() {
    let (_, d) = analyze(&order("read: raw`  `"));
    assert!(errors(&d).contains(&"E0357"), "{:?}", codes(&d));
}

#[test]
fn qualified_id_in_override_is_rejected() {
    let (_, d) = analyze(&order(
        "read: raw`NOT EXISTS (SELECT 1 FROM hold h WHERE h.order_id = {table}.{id})`",
    ));
    assert!(errors(&d).contains(&"E0353"), "{:?}", codes(&d));
}

#[test]
fn id_in_override_on_composite_key_is_rejected() {
    let (_, d) = analyze(
        "@soft_delete(deleted_at)\n@key(device, seq)\nReading { device: text, seq: int, deleted_at: timestamp?\nread: raw`{id} IS NOT NULL AND deleted_at IS NULL`\n}\nshape R from Reading { seq }\nquery q() -> R[] { list Reading; }",
    );
    assert!(errors(&d).contains(&"E0356"), "{:?}", codes(&d));
}
