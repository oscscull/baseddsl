use super::*;

// ---------- H6 write-path sweep (2026-08-21) -------------------------------

/// Regression (H6): a `tx` step reference to a bound create's engine-managed `@scope`
/// column (`$p.org`) must bind the caller's `$ctx` value, not a silent `NULL`. The bound
/// `Project` create engine-sets `org` from `:ctx_org` (never an assign — E0181), so
/// `$p.org` is knowable: it is that same `:ctx_org`, since the whole mutation runs under
/// one `$ctx`. Before the fix codegen emitted `NULL /* $p.org not set … */`, silently
/// writing NULL into a downstream row (or spuriously failing a NOT NULL column). Here
/// `Task.owner_org` is *optional*, so the old bug would have passed as a silent NULL —
/// this asserts the write actually carries the tenant's org, and tracks `$ctx` per caller.
#[tokio::test]
async fn tx_binding_reuses_bound_steps_scope_column_end_to_end() {
    let c = compile_sqlite(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Project { id: Id, org: Org, name: text }
        Task { id: Id, project: Project, owner_org: Org?, title: text }
        shape ProjectCard from Project { name }
        shape TaskRow from Task { title, owner_org = owner_org.id }
        mutation new_project(pn: text, tt: text) -> ProjectCard scoped Tenant {
          tx {
            create Project { name = $pn } as p;
            create Task { project = $p.id, owner_org = $p.org, title = $tt };
          }
        }
        query tasks() -> TaskRow[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(r#"INSERT INTO `org` (`id`, `name`) VALUES ('org-a', 'A');"#)
        .await
        .expect("seed");

    // The nested create runs under tenant org-a: the Task's `owner_org` must be the
    // caller's tenant org (the value the bound Project's scope column engine-set from
    // `$ctx`), never a silent NULL and never some other value.
    let made = call(
        &c,
        &backend,
        "POST",
        "/m/new_project",
        json!({ "pn": "P", "tt": "T" }),
        json!({ "org": "org-a" }),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);

    let tasks = call(&c, &backend, "POST", "/q/tasks", json!({}), json!({})).await;
    assert_eq!(tasks.status, 200, "{:?}", tasks.body);
    let rows = tasks.body.as_array().expect("array");
    assert_eq!(rows.len(), 1, "{:?}", tasks.body);
    let owner = rows[0]["owner_org"].as_str().unwrap_or_else(|| {
        panic!(
            "owner_org is NULL — the scope-column binding regressed: {:?}",
            rows[0]
        )
    });
    assert_eq!(
        owner, "org-a",
        "owner_org must track the caller's `$ctx` tenant org"
    );
}

/// D124 (H6-R1): a bound create's `@created` timestamp — engine-set to `CURRENT_TIMESTAMP`,
/// unknowable at plan time — is read back and reused by a sibling step. The persisted
/// `Event.at` must equal the Ticket's real `created_at` (never NULL, never a different
/// instant): the bound create re-selects its written row, so `$t.created_at` reads the
/// value the database actually wrote.
#[tokio::test]
async fn tx_binding_reuses_bound_creates_engine_timestamp_end_to_end() {
    let c = compile_sqlite(
        r#"
        @created(created_at)
        Ticket { id: Id, created_at: timestamp, subject: text }
        Event { id: Id, ticket: Ticket, at: timestamp, note: text }
        shape TicketRow from Ticket { subject, created_at }
        shape EventRow from Event { note, at, ticket = ticket.id }
        mutation open(subject: text, note: text) -> TicketRow {
          tx {
            create Ticket { subject = $subject } as t;
            create Event { ticket = $t.id, at = $t.created_at, note = $note };
          }
        }
        query events() -> EventRow[];
        query tickets() -> TicketRow[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    let made = call(
        &c,
        &backend,
        "POST",
        "/m/open",
        json!({ "subject": "S", "note": "N" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);

    let tickets = call(&c, &backend, "POST", "/q/tickets", json!({}), json!({})).await;
    let events = call(&c, &backend, "POST", "/q/events", json!({}), json!({})).await;
    let ticket_created = tickets.body[0]["created_at"]
        .as_str()
        .expect("ticket created_at");
    let event_at = events.body[0]["at"].as_str().unwrap_or_else(|| {
        panic!(
            "Event.at is NULL — the timestamp binding regressed: {:?}",
            events.body[0]
        )
    });
    assert_eq!(
        event_at, ticket_created,
        "$t.created_at must be the Ticket's real committed created_at"
    );
}

/// D124 (retired E0268): a `serial` (DB-generated) create bound `as t`, whose id is unknown
/// until the INSERT runs, now binds a sibling FK via `$t.id` — the bound create re-selects
/// its written row (`RETURNING id`), so the DB-generated id threads into the later step.
#[tokio::test]
async fn tx_binding_reaches_a_serial_creates_db_generated_id_end_to_end() {
    let c = compile_sqlite(
        r#"
        Org { id: serial, name: text }
        Note { id: Id, org: Org, body: text }
        shape OrgCard from Org { name }
        shape NoteRow from Note { body, org = org.id }
        mutation setup(name: text, body: text) -> OrgCard {
          tx {
            create Org { name = $name } as o;
            create Note { org = $o.id, body = $body };
          }
        }
        query notes() -> NoteRow[];
        query orgs() -> OrgCard[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    let made = call(
        &c,
        &backend,
        "POST",
        "/m/setup",
        json!({ "name": "Acme", "body": "hi" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);

    let notes = call(&c, &backend, "POST", "/q/notes", json!({}), json!({})).await;
    // The Note's FK carries the DB-generated Org id (a serial integer, id 1).
    let org_fk = &notes.body[0]["org"];
    assert!(
        org_fk.as_i64() == Some(1) || org_fk.as_str() == Some("1"),
        "Note.org must be the serial Org's DB-generated id, got {org_fk:?}"
    );
}

/// D107 (H6): a 3-step `tx` where step 3 references step 1 (`$user.id`), reaching *past*
/// the immediately-prior step — proven live, not just at plan time. Both the Address
/// (step 2) and the Log (step 3) must carry the *same* User id (step 1's app-minted id),
/// so a reference that wrongly bound the nearest prior step, or NULL, would diverge.
#[tokio::test]
async fn three_step_tx_binding_reaches_the_first_step_end_to_end() {
    let c = compile_sqlite(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        Log { id: Id, actor: User, note: text }
        shape UserCard from User { email }
        shape AddressRow from Address { city, user = user.id }
        shape LogRow from Log { note, actor = actor.id }
        mutation signup(email: text, city: text, note: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
            create Log { actor = $user.id, note = $note };
          }
        }
        query addresses() -> AddressRow[];
        query logs() -> LogRow[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    let made = call(
        &c,
        &backend,
        "POST",
        "/m/signup",
        json!({ "email": "a@b.c", "city": "NYC", "note": "welcome" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    assert_eq!(made.body, json!({ "email": "a@b.c" }));

    let addrs = call(&c, &backend, "POST", "/q/addresses", json!({}), json!({})).await;
    let logs = call(&c, &backend, "POST", "/q/logs", json!({}), json!({})).await;
    let addr_user = addrs.body[0]["user"].as_str().expect("address.user");
    let log_actor = logs.body[0]["actor"].as_str().expect("log.actor");
    // Step 2 and step 3 both reference the step-1 User: same id, and it is the created one.
    assert_eq!(
        addr_user, log_actor,
        "step 3 reached a different step than step 2"
    );
    assert_eq!(
        addr_user, "id-0",
        "the reference bound the User's own generated id"
    );
}
