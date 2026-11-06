use super::*;

#[tokio::test]
async fn joined_scope_hides_cross_scope_row_end_to_end() {
    // Proven against a real engine: a query on the *unscoped* `Ticket` reaches the
    // org-*scoped* `Contact` through `raised_by`. Codegen injects `Contact`'s `@scope`
    // into the join `ON` (`contact.org_id = :ctx_org`), so a contact belonging to another
    // org is invisible across the join.
    let c = compile_sqlite(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { id: Id, org: Org, name: text }
        Ticket { id: Id, raised_by: Contact?, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query ticket_by_id(id) -> TicketCard scoped Tenant;
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`) VALUES ('org-1', 'Acme'), ('org-2', 'Beta');
            INSERT INTO `contact` (`id`, `org_id`, `name`) VALUES ('c-2', 'org-2', 'Zoe');
            INSERT INTO `ticket` (`id`, `raised_by_id`, `subject`)
                VALUES ('t-1', 'c-2', 'help');
            "#,
        )
        .await
        .expect("seed");

    // Caller is in org-1; the ticket's contact belongs to org-2. The `LEFT JOIN` still
    // yields the ticket row, but the scoped join means the cross-org contact is filtered
    // out — `who` comes back null (the joined name is invisible across the scope boundary).
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/ticket_by_id",
        json!({ "id": "t-1" }),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!({ "subject": "help", "who": null }));

    // The contact's own org *does* see the joined name — same request, in-scope caller.
    let in_scope = call(
        &c,
        &backend,
        "POST",
        "/q/ticket_by_id",
        json!({ "id": "t-1" }),
        json!({ "org": "org-2" }),
    )
    .await;
    assert_eq!(in_scope.body, json!({ "subject": "help", "who": "Zoe" }));
}

#[tokio::test]
async fn delete_all_wipes_the_table_end_to_end() {
    // `hard delete all` -> every row gone (SQLite's DELETE-FROM-no-WHERE truncate path),
    // and `-> ok` succeeds even against an already-empty table (no 404 for a wipe).
    let c = compile_sqlite(
        r#"
        Widget { id: Id, name: text }
        shape WidgetRow from Widget { name }
        query all_widgets() -> WidgetRow[] { list Widget; }
        mutation wipe() -> ok { hard delete all Widget; }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend.execute_batch(&ddl).await.expect("ddl");
    backend
        .execute_batch(r#"INSERT INTO `widget` (`id`, `name`) VALUES ('w-1', 'A'), ('w-2', 'B');"#)
        .await
        .expect("seed");

    let before = call(&c, &backend, "POST", "/q/all_widgets", json!({}), json!({})).await;
    assert_eq!(before.body.as_array().map(Vec::len), Some(2));

    let wiped = call(&c, &backend, "POST", "/m/wipe", json!({}), json!({})).await;
    assert_eq!(wiped.status, 200, "{:?}", wiped.body);
    assert_eq!(wiped.body, json!({}), "wipe acks with an empty body");

    let after = call(&c, &backend, "POST", "/q/all_widgets", json!({}), json!({})).await;
    assert_eq!(after.body, json!([]), "every row is gone");

    // A second wipe of the now-empty table is still a success (no absent-row 404).
    let again = call(&c, &backend, "POST", "/m/wipe", json!({}), json!({})).await;
    assert_eq!(
        again.status, 200,
        "wipe of an empty table succeeds: {:?}",
        again.body
    );
}

#[tokio::test]
async fn scoped_delete_all_wipes_only_the_callers_scope_end_to_end() {
    // A scoped `hard delete all` is all rows *in the caller's scope* — the scope predicate
    // still rides the DELETE, so another tenant's rows survive.
    let c = compile_sqlite(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Invoice { id: Id, org: Org, memo: text, @index(org) }
        shape InvoiceRow from Invoice { memo }
        query my_invoices() -> InvoiceRow[] scoped Tenant { list Invoice; }
        mutation wipe_mine() -> ok scoped Tenant { hard delete all Invoice; }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("ddl");
    backend
        .execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`) VALUES ('org-1', 'Acme'), ('org-2', 'Beta');
            INSERT INTO `invoice` (`id`, `org_id`, `memo`) VALUES
                ('i-1', 'org-1', 'mine'), ('i-2', 'org-2', 'theirs');
            "#,
        )
        .await
        .expect("seed");

    let wiped = call(
        &c,
        &backend,
        "POST",
        "/m/wipe_mine",
        json!({}),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(wiped.status, 200, "{:?}", wiped.body);

    // org-1 sees nothing; org-2's row is untouched.
    let mine = call(
        &c,
        &backend,
        "POST",
        "/q/my_invoices",
        json!({}),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(mine.body, json!([]), "caller's scope wiped");
    let theirs = call(
        &c,
        &backend,
        "POST",
        "/q/my_invoices",
        json!({}),
        json!({ "org": "org-2" }),
    )
    .await;
    assert_eq!(
        theirs.body,
        json!([{ "memo": "theirs" }]),
        "other scope survives"
    );
}

#[tokio::test]
async fn soft_delete_all_tombstones_every_row_end_to_end() {
    // Soft model + `delete all` -> tombstone every live row (an UPDATE), never a real
    // DELETE. The live query then sees nothing; `hard delete all` would physically remove.
    let c = compile_sqlite(
        r#"
        @soft_delete(deleted_at)
        Note { id: Id, deleted_at: timestamp?, body: text }
        shape NoteRow from Note { body }
        query live_notes() -> NoteRow[] { list Note; }
        mutation archive_all() -> ok { delete all Note; }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("ddl");
    backend
        .execute_batch(r#"INSERT INTO `note` (`id`, `body`) VALUES ('n-1', 'x'), ('n-2', 'y');"#)
        .await
        .expect("seed");

    let archived = call(&c, &backend, "POST", "/m/archive_all", json!({}), json!({})).await;
    assert_eq!(archived.status, 200, "{:?}", archived.body);

    // No live rows remain — every row was tombstoned (an UPDATE, not a physical DELETE;
    // the tombstone-vs-real-delete SQL is asserted in the codegen tests).
    let live = call(&c, &backend, "POST", "/q/live_notes", json!({}), json!({})).await;
    assert_eq!(live.body, json!([]), "all rows tombstoned");
}

#[tokio::test]
async fn delete_and_update_across_relation_run_on_sqlite_end_to_end() {
    // A `where` reaching through a to-one relation lowers to forms SQLite accepts
    // (correlated EXISTS for delete, UPDATE … FROM for update), touching only the rows
    // whose deck belongs to the given user.
    let c = compile_sqlite(
        r#"
        User { id: Id, name: text }
        Deck { id: Id, user: User, @index(user) }
        DeckCard { id: Id, deck: Deck, name: text, @index(deck) }
        shape CardRow from DeckCard { name }
        query all_cards() -> CardRow[] { list DeckCard; }
        mutation rename_user_cards(user: User, name: text) -> ok {
          update DeckCard where (deck.user = $user) { name = $name };
        }
        mutation clear_user_cards(user: User) -> ok {
          delete DeckCard where (deck.user = $user);
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("ddl");
    let u1 = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    let u2 = "3f2504e0-4f89-41d3-9a0c-0305e82c3302";
    backend
        .execute_batch(&format!(
            r#"
            INSERT INTO `user` (`id`, `name`) VALUES ('{u1}', 'Ada'), ('{u2}', 'Bea');
            INSERT INTO `deck` (`id`, `user_id`) VALUES ('d-1', '{u1}'), ('d-2', '{u2}');
            INSERT INTO `deck_card` (`id`, `deck_id`, `name`) VALUES
                ('c-1', 'd-1', 'a'), ('c-2', 'd-1', 'b'), ('c-3', 'd-2', 'keep');
            "#
        ))
        .await
        .expect("seed");

    let renamed = call(
        &c,
        &backend,
        "POST",
        "/m/rename_user_cards",
        json!({ "user": u1, "name": "renamed" }),
        json!({}),
    )
    .await;
    assert_eq!(renamed.status, 200, "{:?}", renamed.body);
    let cards = call(&c, &backend, "POST", "/q/all_cards", json!({}), json!({})).await;
    let mut names: Vec<String> = cards
        .body
        .as_array()
        .expect("card list")
        .iter()
        .map(|r| r["name"].as_str().expect("name").to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["keep", "renamed", "renamed"]);

    let cleared = call(
        &c,
        &backend,
        "POST",
        "/m/clear_user_cards",
        json!({ "user": u1 }),
        json!({}),
    )
    .await;
    assert_eq!(cleared.status, 200, "{:?}", cleared.body);
    let after = call(&c, &backend, "POST", "/q/all_cards", json!({}), json!({})).await;
    assert_eq!(after.body, json!([{ "name": "keep" }]));
}
