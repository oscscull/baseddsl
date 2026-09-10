use super::*;

// ---------- whole-query raw bodies (raw.md's third level) -------------------

#[test]
fn raw_query_body_with_typed_params_is_clean() {
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        User { id: Id, org: Org, name: text, email: text }
        shape UserRow from User { name, email }
        query heavy_users(min: int) -> UserRow[] {
          raw`SELECT u.name AS name, u.email AS email FROM user u WHERE u.id >= ${min}`;
        }
        "#,
    );
    assert!(d.is_empty(), "{:?}", codes(&d));
    let q = &schema.queries[0];
    assert_eq!(q.target, "User");
    assert_eq!(q.verb, Verb::List);
    assert!(q.many && !q.paginated);
}

#[test]
fn raw_query_scalar_return_needs_no_unique_key() {
    // A raw `get` skips E0144 — the SQL owns its keying.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query one(who: text) -> UserRow { raw`SELECT name FROM user WHERE name = ${who}`; }
        "#,
    );
    assert!(d.is_empty(), "{:?}", codes(&d));
}

#[test]
fn raw_query_param_must_be_typed() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query heavy(min) -> UserRow[] { raw`SELECT name FROM user WHERE id >= ${min}`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0210"), "{:?}", codes(&d));
}

#[test]
fn raw_query_param_binding_is_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text, created_at: timestamp }
        shape UserRow from User { name }
        query heavy(since: timestamp > created_at) -> UserRow[] { raw`SELECT name FROM user`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0210"), "{:?}", codes(&d));
}

#[test]
fn raw_query_unknown_param_is_reported() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query heavy() -> UserRow[] { raw`SELECT name FROM user WHERE id = ${nope}`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0113"), "{:?}", codes(&d));
}

#[test]
fn raw_query_ctx_ref_is_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query mine() -> UserRow[] { raw`SELECT name FROM user WHERE org = ${ctx.org}`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0214"), "{:?}", codes(&d));
}

#[test]
fn raw_query_cannot_stream() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape UserRow from User { name }
        query all() -> stream UserRow { raw`SELECT name FROM user`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0212"), "{:?}", codes(&d));
}

#[test]
fn raw_query_on_scoped_model_must_be_unscoped() {
    // `scoped` promises an injection the engine can't perform in raw SQL (E0211);
    // `unscoped("…")` is the loud, legal spelling.
    let src_scoped = r#"
        scope Tenant (org: Org = $ctx.org)
        Org { id: Id, name: text }
        @scope Tenant
        Ticket { id: Id, org: Org, title: text }
        shape TicketRow from Ticket { title }
        query all() -> TicketRow[] scoped Tenant { raw`SELECT title FROM ticket`; }
    "#;
    let (_, d) = analyze(src_scoped);
    assert!(errors(&d).contains(&"E0211"), "{:?}", codes(&d));

    let src_unscoped = r#"
        scope Tenant (org: Org = $ctx.org)
        Org { id: Id, name: text }
        @scope Tenant
        Ticket { id: Id, org: Org, title: text }
        shape TicketRow from Ticket { title }
        query all() -> TicketRow[] unscoped("admin report") { raw`SELECT title FROM ticket`; }
    "#;
    let (_, d) = analyze(src_unscoped);
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));

    // Writing neither is the ordinary missing-ack error.
    let src_bare = r#"
        scope Tenant (org: Org = $ctx.org)
        Org { id: Id, name: text }
        @scope Tenant
        Ticket { id: Id, org: Org, title: text }
        shape TicketRow from Ticket { title }
        query all() -> TicketRow[] { raw`SELECT title FROM ticket`; }
    "#;
    let (_, d) = analyze(src_bare);
    assert!(errors(&d).contains(&"E0182"), "{:?}", codes(&d));
}

#[test]
fn raw_query_shape_must_be_flat() {
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        User { id: Id, org: Org, name: text }
        shape UserCard from User { name, org { name } }
        query all() -> UserCard[] { raw`SELECT name FROM user`; }
        "#,
    );
    assert!(errors(&d).contains(&"E0213"), "{:?}", codes(&d));
}

#[test]
fn raw_query_soft_delete_gap_is_linted() {
    // The target model's tombstone, and any other soft-delete table the raw text
    // mentions, each get a W0102 — never silent.
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp?, user: User, total: int }
        shape UserRow from User { name }
        query buyers() -> UserRow[] {
          raw`SELECT u.name AS name FROM user u JOIN order o ON o.user_id = u.id WHERE u.deleted_at IS NULL`;
        }
        "#,
    );
    let warns = d
        .iter()
        .filter(|x| x.severity == Severity::Warning && x.code == "W0102")
        .count();
    assert_eq!(warns, 2, "{:?}", codes(&d));
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}
