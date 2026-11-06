use super::*;

// ---------- $ctx inference + coherence  -----------------------------

#[test]
fn ctx_inferred_from_use_is_clean() {
    // No declaration anywhere: `$ctx.org`'s type is inferred from the `org` column
    // it compares against.
    assert_clean(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx.org) order (title); }
        "#,
    );
}

#[test]
fn ctx_requirement_is_recorded_per_callable() {
    // The inferred requirement is attached to the callable that reads it — the
    // client sends exactly this as request context.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx.org) order (title); }
        query all() -> D[] order (title);
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let docs = schema.queries.iter().find(|q| q.name == "docs").unwrap();
    assert_eq!(docs.ctx_requires.len(), 1);
    assert_eq!(docs.ctx_requires[0].field, "org");
    // a query that reads no context requires none
    let all = schema.queries.iter().find(|q| q.name == "all").unwrap();
    assert!(all.ctx_requires.is_empty());
}

#[test]
fn ctx_scope_propagates_to_every_query() {
    // `@scope` reads `$ctx.org`, so every query on the model requires it even with
    // no `where` of its own.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant order (title);
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let docs = schema.queries.iter().find(|q| q.name == "docs").unwrap();
    assert_eq!(docs.ctx_requires.len(), 1);
    assert_eq!(docs.ctx_requires[0].field, "org");
}

#[test]
fn ctx_joined_scope_is_required_via_shape_reach() {
    // a query on an *unscoped* model that reaches a *scoped* model through a
    // shape relation joins it, and codegen injects the joined model's `@scope` into
    // the join `ON` — so the callable must require that model's `$ctx.org`, else the
    // injected `:ctx_org` bind is unbound at runtime.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query ticket_by_id(id) -> TicketCard scoped Tenant;
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "ticket_by_id")
        .unwrap();
    assert_eq!(q.ctx_requires.len(), 1, "{:?}", q.ctx_requires);
    assert_eq!(q.ctx_requires[0].field, "org");
}

#[test]
fn ctx_joined_scope_is_required_via_where_reach() {
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text, @index name }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject }
        query tickets(name) -> TicketCard[] scoped Tenant {
          list Ticket where (raised_by.name = $name) order (subject);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema.queries.iter().find(|q| q.name == "tickets").unwrap();
    assert!(
        q.ctx_requires.iter().any(|r| r.field == "org"),
        "{:?}",
        q.ctx_requires
    );
}

#[test]
fn ctx_unscoped_query_drops_joined_scope_requirement() {
    // `unscoped`  drops all scope handling, joins included, so the joined
    // scoped model contributes no `$ctx` requirement (mirrors codegen).
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query any_ticket(id) -> TicketCard unscoped("admin: cross-org lookup");
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "any_ticket")
        .unwrap();
    assert!(q.ctx_requires.is_empty(), "{:?}", q.ctx_requires);
}

#[test]
fn ctx_bad_path_errors() {
    // `$ctx` fields are flat: exactly one segment.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx.org.deep) order (title); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0160"]);
}

#[test]
fn ctx_bare_no_field_errors() {
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query docs() -> D[] { list Doc where (org = $ctx) order (title); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0160"]);
}

#[test]
fn ctx_coherent_across_callables_is_clean() {
    // `$ctx.org` is an `Org` key in both queries — one coherent request-context bag.
    assert_clean(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org }
        shape D from Doc { title }
        query a() -> D[] { list Doc where (org = $ctx.org) order (title); }
        query b() -> D[] { list Doc where (org = $ctx.org) order (title); }
        "#,
    );
}

#[test]
fn ctx_conflict_across_callables_errors() {
    // `$ctx.org` is an `Org` key in `a` but a text value in `b` — the caller can't
    // build one bag that satisfies both.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org, @index title }
        shape D from Doc { title }
        query a() -> D[] { list Doc where (org = $ctx.org) order (title); }
        query b() -> D[] { list Doc where (title = $ctx.org) order (title); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0161"]);
}

#[test]
fn ctx_conflict_within_one_callable_errors() {
    // Same field used at two types in one query is itself incoherent.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text, @index org, @index title }
        shape D from Doc { title }
        query a() -> D[] {
          list Doc where (org = $ctx.x and title = $ctx.x) order (title);
        }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0161"]);
}

#[test]
fn ctx_model_key_matches_that_models_own_id() {
    // A `User` key compared with `User.id` is the same value as the scope's `User` key, so
    // the mutation requires `$ctx.user` once, still typed as the model key.
    let (schema, d) = analyze(
        r#"
        scope Owner (user: User = $ctx.user)
        User { id: uuid  session_version: int }
        @scope Owner
        Deck { id: uuid  user: User  @index(user) }

        mutation drop_me() -> ok scoped Owner {
          tx {
            delete Deck where (user = $ctx.user);
            delete User where (id = $ctx.user);
          }
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let m = schema
        .mutations
        .iter()
        .find(|m| m.name == "drop_me")
        .unwrap();
    let user: Vec<_> = m
        .ctx_requires
        .iter()
        .filter(|r| r.field == "user")
        .collect();
    assert_eq!(user.len(), 1, "{:?}", m.ctx_requires);
    assert!(
        matches!(&user[0].ty, based_sema::CtxField::Relation(t) if t == "User"),
        "{:?}",
        user[0].ty
    );
}

#[test]
fn ctx_own_id_use_agrees_with_model_key_across_callables() {
    // A callable that reads `$ctx.user` only through `User.id` keeps its scalar type and
    // still agrees with the `User` key another callable reads.
    let (schema, d) = analyze(
        r#"
        User { id: Id, name: text }
        Doc { id: Id, owner: User, title: text, @index owner }
        shape D from Doc { title }
        shape U from User { name }
        query mine() -> D[] { list Doc where (owner = $ctx.user) order (title); }
        query me() -> U { get User where (id = $ctx.user); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let me = schema.queries.iter().find(|q| q.name == "me").unwrap();
    assert!(
        matches!(me.ctx_requires[..], [ref r] if matches!(r.ty, based_sema::CtxField::Scalar(_))),
        "{:?}",
        me.ctx_requires
    );
}

#[test]
fn ctx_own_id_through_a_relation_matches_model_key() {
    // `owner.id` is still `User`'s whole key.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        Doc { id: Id, owner: User, title: text, @index owner }
        shape D from Doc { title }
        query a() -> D[] { list Doc where (owner = $ctx.user) order (title); }
        query b() -> D[] { list Doc where (owner.id = $ctx.user) order (title); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn ctx_serial_and_natural_keys_match_model_key() {
    let (_, d) = analyze(
        r#"
        User { id: serial, name: text }
        @key(code)
        Team { code: text, name: text }
        Doc { id: Id, owner: User, team: Team, title: text, @index owner, @index team }
        shape D from Doc { title }
        shape U from User { name }
        shape T from Team { name }
        query mine() -> D[] { list Doc where (owner = $ctx.user) order (title); }
        query ours() -> D[] { list Doc where (team = $ctx.team) order (title); }
        query me() -> U { get User where (id = $ctx.user); }
        query my_team() -> T { get Team where (code = $ctx.team); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn ctx_other_models_id_conflicts_with_model_key() {
    // `Doc.id` is a `Doc` key, never a `User` key, though both are uuids.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        Doc { id: Id, owner: User, title: text, @index owner }
        shape D from Doc { title }
        query a() -> D[] { list Doc where (owner = $ctx.user) order (title); }
        query b() -> D { get Doc where (id = $ctx.user); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0161"]);
}

#[test]
fn ctx_non_key_uuid_column_conflicts_with_model_key() {
    // A uuid column that is not the model's key stays a plain `uuid`.
    let (_, d) = analyze(
        r#"
        User { id: Id, token: uuid, name: text, @index token }
        Doc { id: Id, owner: User, title: text, @index owner }
        mutation drop_mine() -> ok {
          tx {
            delete Doc where (owner = $ctx.user);
            delete User where (token = $ctx.user);
          }
        }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0161"]);
}

#[test]
fn ctx_composite_key_part_conflicts_with_model_key() {
    // One part of a composite key is not the model's whole key.
    let (_, d) = analyze(
        r#"
        Tenant { id: Id, name: text }
        @key(tenant, code)
        Member { tenant: Tenant  code: uuid  name: text }
        Seat { id: Id, member: Member, label: text, @index member }
        shape S from Seat { label }
        shape M from Member { name }
        query a() -> S[] { list Seat where (member = $ctx.m) order (label); }
        query b() -> M[] { list Member where (code = $ctx.m) order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0161"), "{:?}", codes(&d));
}

#[test]
fn ctx_from_create_assign_is_recorded() {
    // A `create` can set a column from context; the field types from that column.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text }
        shape D from Doc { title }
        mutation add(t: text) -> D { create Doc { org = $ctx.org, title = $t }; }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let add = schema.mutations.iter().find(|m| m.name == "add").unwrap();
    assert_eq!(add.ctx_requires.len(), 1);
    assert_eq!(add.ctx_requires[0].field, "org");
}

#[test]
fn ctx_mutation_reselect_joined_scope_is_required() {
    // a mutation's declared-shape re-select  projects the return shape, so a
    // relation reach in that shape joins a scoped model and injects its `@scope` — the
    // mutation must require the joined model's `$ctx.org` too. Here `Ticket` is
    // unscoped but its re-select reaches the org-scoped `Contact`.
    let (schema, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        mutation open_ticket(by: Contact, subject: text) -> TicketCard scoped Tenant {
          create Ticket { raised_by = $by, subject = $subject };
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let m = schema
        .mutations
        .iter()
        .find(|m| m.name == "open_ticket")
        .unwrap();
    assert!(
        m.ctx_requires.iter().any(|r| r.field == "org"),
        "{:?}",
        m.ctx_requires
    );
}
