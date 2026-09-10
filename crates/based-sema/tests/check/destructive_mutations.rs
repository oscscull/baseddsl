use super::*;

// ---------- `-> ok`: the destructive-mutation acknowledgement ----------------

#[test]
fn ack_hard_delete_resolves_the_deleted_model() {
    let (schema, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Comment { id: Id, deleted_at: timestamp?, body: text }
        mutation purge_comment(id: Id) -> ok {
          hard delete Comment where (id = $id);
        }
        "#,
    );
    assert!(d.is_empty(), "{:?}", codes(&d));
    let m = &schema.mutations[0];
    assert!(m.ack);
    assert_eq!(m.ret_model, "Comment");
    assert_eq!(m.ret_shape, None);
}

#[test]
fn ack_plain_delete_on_plain_model_is_clean() {
    let (schema, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        mutation drop_tag(id: Id) -> ok {
          delete Tag where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    assert!(schema.mutations[0].ack);
}

#[test]
fn ack_hard_delete_all_wipes_the_table() {
    let (schema, d) = analyze(
        r#"
        Widget { id: Id, name: text }
        mutation wipe() -> ok {
          hard delete all Widget;
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    assert!(schema.mutations[0].ack);
    assert_eq!(schema.mutations[0].ret_model, "Widget");
}

#[test]
fn ack_soft_delete_all_is_a_tombstone_wipe_not_a_surviving_write() {
    // A soft `delete all` tombstones every row — no single row to read back, so `-> ok`
    // is valid (it does NOT trip E0221 the way a filtered soft `delete` with `-> ok` would).
    let (schema, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp? }
        mutation archive_all() -> ok {
          delete all Order;
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    assert!(schema.mutations[0].ack);
}

#[test]
fn shape_on_soft_delete_all_is_rejected() {
    // No single surviving row to read back from a whole-table tombstone → E0220.
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp?, total: int }
        shape OrderRow from Order { total }
        mutation archive_all() -> OrderRow {
          delete all Order;
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0220"), "{:?}", codes(&d));
}

#[test]
fn bare_delete_without_where_or_all_is_a_parse_error() {
    // `delete Model` with neither a `where` clause nor the `all` keyword must not parse —
    // "delete everything" is only ever the explicit, greppable `all`.
    let sf = parse_file("mutation m() -> ok { delete Widget; }", FileId(0));
    assert!(sf.is_err(), "a bare `delete Model` must be a parse error");
}

#[test]
fn shape_on_real_delete_is_rejected() {
    // No surviving row to read back as the shape → E0220.
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        shape TagCard from Tag { label }
        mutation drop_tag(id: Id) -> TagCard {
          delete Tag where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0220"), "{:?}", codes(&d));
}

#[test]
fn shape_on_hard_delete_is_rejected() {
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Comment { id: Id, deleted_at: timestamp?, body: text }
        shape CommentRow from Comment { body }
        mutation purge_comment(id: Id) -> CommentRow {
          hard delete Comment where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0220"), "{:?}", codes(&d));
}

#[test]
fn shape_survives_when_a_tx_sibling_creates_the_return_row() {
    // The delete removes one model's row, but the return model is re-created by a
    // sibling write — a surviving row exists, so the declared shape stands.
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        Audit { id: Id, note: text }
        shape AuditRow from Audit { note }
        mutation drop_tag(id: Id, note: text) -> AuditRow {
          tx {
            delete Tag where (id = $id);
            create Audit { note = $note };
          }
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn ack_on_soft_delete_is_allowed_universal_optout() {
    // BW1 broadened `-> ok` to a universal opt-out of read-back — a surviving write
    // (here a soft `delete` tombstone) under `-> ok` is legal, no longer E0221.
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        Comment { id: Id, deleted_at: timestamp?, body: text }
        mutation remove_comment(id: Id) -> ok {
          delete Comment where (id = $id);
        }
        "#,
    );
    assert!(!errors(&d).contains(&"E0221"), "{:?}", codes(&d));
}

#[test]
fn ack_on_create_or_update_is_allowed_universal_optout() {
    // A `create`/`update` may opt out of its declared-shape read-back with `-> ok`.
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        mutation rename_tag(id: Id, label: text) -> ok {
          update Tag where (id = $id) { label = $label };
        }
        "#,
    );
    assert!(!errors(&d).contains(&"E0221"), "{:?}", codes(&d));
}

#[test]
fn ack_without_a_real_delete_is_rejected() {
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        mutation noop() -> ok {
          raw`ANALYZE`;
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0221"), "{:?}", codes(&d));
}

#[test]
fn ack_on_a_query_is_rejected() {
    let (_, d) = analyze(
        r#"
        Tag { id: Id, label: text }
        query tags() -> ok;
        "#,
    );
    assert!(errors(&d).contains(&"E0222"), "{:?}", codes(&d));
}

#[test]
fn ack_scoped_hard_delete_keeps_scope_ack_checking() {
    // The ack mutation still owes the scope acknowledgement for the model it deletes.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Comment { id: Id, org: Org, body: text }
        mutation purge_comment(id: Id) -> ok {
          hard delete Comment where (id = $id);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0182"), "{:?}", codes(&d));
}
