//! End-to-end integration against a **real** engine (SQLite), no mock (D27).
//!
//! Every other runtime test drives the plan → run → shape path against a `MockDb` that
//! returns canned rows — so it proves the *binding* is right but never that the emitted
//! SQL actually executes. This test closes that gap: it loads the real commerce schema
//! (`Compiled::load`), seeds an in-memory SQLite database, and dispatches real requests
//! through `serve::dispatch` against the concrete `SqliteDb`/`SqliteBackend`. What runs is
//! the *verbatim* codegen-lowered SQL (`based gen sql`), bound positionally by the runtime
//! — so a passing test means the whole engine works against a genuine database, and that
//! the `Db`/`Backend`/`ping` seams are real, not just compile-verified.
//!
//! It needs no infra (bundled in-memory SQLite), so it runs in CI like any unit test.
//!
//! SQLite accepts the runtime's DML as-is (backtick identifiers, `= TRUE`, `IS NULL`,
//! joins, `LIMIT`, positional `?`). As of D28 the **DDL** is also generated for SQLite —
//! the tables here are created from `based gen sql`'s SQLite output (`sql::ddl` with
//! `Dialect::Sqlite`) run against the loaded schema, so the whole `based gen sql` artifact
//! (DDL *and* DML) is now proven to execute, not just the query text.

#![cfg(feature = "sqlite")]

use std::path::PathBuf;

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::run::Backend;
use based_runtime::{dispatch, Compiled, SeqIdGen, SqliteBackend};
use based_sema::check;

/// Load the real commerce example — the same front end (discover → parse → check) +
/// codegen lowering the CLI uses, so the SQL executed here is `based gen sql`'s output.
fn commerce() -> Compiled {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/examples/commerce")
        .canonicalize()
        .expect("commerce example dir");
    Compiled::load(&root).unwrap_or_else(|e| panic!("commerce did not load: {e:?}"))
}

/// Seed an in-memory SQLite database: create every commerce table from the *generated*
/// SQLite DDL (`based gen sql` with `Dialect::Sqlite`, D28), then insert a couple of rows.
/// Running the real DDL — not a hand-shaped copy — means this test now exercises the whole
/// `based gen sql` artifact end to end: the DDL creates the schema the DML then reads/writes.
fn seeded_backend(c: &Compiled) -> SqliteBackend {
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .unwrap_or_else(|e| panic!("generated SQLite DDL failed to execute: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`, `slug`) VALUES ('org-1', 'Acme', 'acme');
            INSERT INTO `user` (`id`, `email`, `name`) VALUES ('user-1', 'a@x.com', 'Ada');
            INSERT INTO `order` (`id`, `org_id`, `placed_by_id`, `status`, `total`)
                VALUES ('order-1', 'org-1', 'user-1', 'paid', 500);
            "#,
        )
        .expect("seed fixtures");
    backend
}

/// Run one request through the real dispatch core against a checked-out `SqliteDb`.
fn call(
    compiled: &Compiled,
    backend: &SqliteBackend,
    method: &str,
    path: &str,
    args: serde_json::Value,
    ctx: serde_json::Value,
) -> based_runtime::WireResponse {
    let mut db = backend.checkout("").expect("checkout");
    let mut ids = SeqIdGen::default();
    dispatch(
        compiled,
        db.as_mut(),
        &mut ids,
        &NoStore,
        method,
        path,
        args,
        ctx,
        None,
    )
}

#[test]
fn get_query_runs_against_real_sqlite() {
    // `order_by_id` is a `get`: it joins order → user + org and projects the OrderCard
    // shape. This is the verbatim lowered SELECT executed against a live SQLite row.
    let c = commerce();
    let backend = seeded_backend(&c);
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "order-1" }),
        // Order is `@scope`d (D32): even a keyed `get` is org-scoped, so `$ctx.org` is
        // required. order-1 belongs to org-1, so it's visible to this caller.
        json!({ "org": "org-1" }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "paid", "total": 500, "buyer": "Ada", "org": "Acme" })
    );
}

#[test]
fn get_query_misses_return_null() {
    // A `get` on an absent key is `Option<T>` → JSON null (the envelope, realized by a
    // real empty result set, not a canned one).
    let c = commerce();
    let backend = seeded_backend(&c);
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "nope" }),
        json!({ "org": "org-1" }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!(null));
}

#[test]
fn ctx_scoped_list_query_binds_context() {
    // `my_org_orders` reads `$ctx.org` — the server supplies it out of band, and the
    // runtime binds it positionally into the WHERE. A `list` shapes as a JSON array.
    let c = commerce();
    let backend = seeded_backend(&c);
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-1" }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!([{ "status": "paid", "total": 500, "buyer": "Ada", "org": "Acme" }])
    );

    // A different org sees none of org-1's rows — the injected scope predicate is real.
    let empty = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-other" }),
    );
    assert_eq!(empty.body, json!([]));
}

#[test]
fn mutation_writes_then_reselects_declared_shape() {
    // `place_order` creates an Order (engine-generated id) and reads it back in its
    // declared OrderCard shape (D12), all under one transaction — the full write path
    // against a real engine: INSERT executes, the re-select joins and projects.
    let c = commerce();
    let backend = seeded_backend(&c);
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/place_order",
        // `org` is `@scope`-managed on create (D32): supplied via `$ctx`, auto-set on the
        // INSERT — never a body arg. The re-select projects `org.name` = "Acme" (org-1).
        json!({ "buyer": "user-1", "total": 99 }),
        json!({ "org": "org-1" }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    // The response is the created row in its declared shape (status defaults to 'pending').
    assert_eq!(
        resp.body,
        json!({ "status": "pending", "total": 99, "buyer": "Ada", "org": "Acme" })
    );

    // The write actually committed: the new order is now visible to a read.
    let listed = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-1" }),
    );
    let rows = listed.body.as_array().expect("list");
    assert_eq!(
        rows.len(),
        2,
        "the created order is now readable: {:?}",
        rows
    );
}

#[test]
fn bad_arg_is_a_400_before_sql() {
    // A mistyped arg is a boundary error caught before any SQL touches SQLite.
    let c = commerce();
    let backend = seeded_backend(&c);
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/place_order",
        json!({ "buyer": "user-1", "total": "not-an-int" }),
        json!({ "org": "org-1" }),
    );
    assert_eq!(resp.status, 400, "{:?}", resp.body);
    assert_eq!(resp.body["error"]["code"], json!("bad_arg"));
}

#[test]
fn backend_ping_succeeds_on_a_live_db() {
    // The readiness seam (D26) works against a real engine: `SELECT 1` round-trips.
    let c = commerce();
    assert!(seeded_backend(&c).ping().is_ok());
}

/// Compile an in-line schema for SQLite (skip disk), mirroring `Compiled::load`.
fn compile_sqlite(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    Compiled::from_checked(schema, sf.decls, Dialect::Sqlite)
}

#[test]
fn joined_scope_hides_cross_scope_row_end_to_end() {
    // D34, proven against a real engine: a query on the *unscoped* `Ticket` reaches the
    // org-*scoped* `Contact` through `raised_by`. Codegen injects `Contact`'s `@scope`
    // into the join `ON` (`contact.org_id = :ctx_org`), so a contact belonging to another
    // org is invisible across the join. This is the exact cross-scope leak D32 left open.
    let c = compile_sqlite(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Contact { org: Org, name: text }
        Ticket { raised_by: Contact?, subject: text }
        shape TicketCard from Ticket { subject, who = raised_by.name }
        query ticket_by_id(id) -> TicketCard scoped Tenant;
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
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
    );
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
    );
    assert_eq!(in_scope.body, json!({ "subject": "help", "who": "Zoe" }));
}

/// A to-one nested shape sub-object (`placed_by { name, email }`, L1) returns a nested
/// JSON object end-to-end: the codegen-prefixed columns (`placed_by.name`, …) come back
/// from the live SELECT and the runtime reassembles them into a sub-object — proven
/// against a real engine, not compile-verified. Self-contained (no commerce schema) so
/// the nesting is the only variable.
#[test]
fn nested_to_one_query_returns_nested_json() {
    let src = r#"
        User { name: text, email: text }
        @sort(id asc)
        Order { placed_by: User, fulfilled_by: User?, total: int }
        shape OrderCard from Order { total, placed_by { name, email } }
        query order_by_id(id) -> OrderCard;
        query orders() -> OrderCard[];
    "#;
    let sf = parse_file(src, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != based_diagnostics::Severity::Error),
        "unexpected sema errors: {diags:#?}"
    );
    let c = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .unwrap_or_else(|e| panic!("generated DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `email`) VALUES ('u1', 'Ada', 'a@x.com');
            INSERT INTO `order` (`id`, `placed_by_id`, `total`) VALUES ('o1', 'u1', 500);
            "#,
        )
        .expect("seed");

    // `get`: the nested object rides back under `placed_by`, not as flat `placed_by.*`.
    let mut db = backend.checkout("").expect("checkout");
    let mut ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        db.as_mut(),
        &mut ids,
        &NoStore,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
        None,
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "total": 500, "placed_by": { "name": "Ada", "email": "a@x.com" } })
    );

    // `list`: every row reassembles independently.
    let mut db = backend.checkout("").expect("checkout");
    let listed = dispatch(
        &c,
        db.as_mut(),
        &mut ids,
        &NoStore,
        "POST",
        "/q/orders",
        json!({}),
        json!({}),
        None,
    );
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([{ "total": 500, "placed_by": { "name": "Ada", "email": "a@x.com" } }])
    );
}

/// Keyset-cursor pagination (L2), proven against a real engine: paging a `page (2)`
/// keyset query walks the whole set exactly once — each page returns the next window and
/// an opaque cursor, the final short page returns a `null` cursor, and the cursor works
/// even though the sort basis (`rank`, `id`) is not projected (the runtime strips the
/// hidden `__keyset_*` columns from the response). A tampered cursor is a 400.
#[test]
fn keyset_pagination_walks_the_set_end_to_end() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Item { name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query items() -> ItemCard[] { list Item order (rank asc) page (2); }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `item` (`id`, `name`, `rank`) VALUES
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30),
                ('i4', 'd', 40), ('i5', 'e', 50);
            "#,
        )
        .expect("seed");

    let page = |args: serde_json::Value| call(&c, &backend, "POST", "/q/items", args, json!({}));

    // Page 1 (no cursor): the two lowest-ranked rows + a "more" cursor (a full page).
    // Rows carry only the projected `{name, rank}` — the hidden sort-key columns are
    // stripped even though they drive the cursor.
    let p1 = page(json!({}));
    assert_eq!(p1.status, 200, "{:?}", p1.body);
    assert_eq!(
        p1.body["rows"],
        json!([{ "name": "a", "rank": 10 }, { "name": "b", "rank": 20 }])
    );
    let c1 = p1.body["cursor"]
        .as_str()
        .expect("page 1 cursor")
        .to_string();

    // Page 2 (cursor from page 1): the next window, another full page → another cursor.
    let p2 = page(json!({ "cursor": c1 }));
    assert_eq!(
        p2.body["rows"],
        json!([{ "name": "c", "rank": 30 }, { "name": "d", "rank": 40 }])
    );
    let c2 = p2.body["cursor"]
        .as_str()
        .expect("page 2 cursor")
        .to_string();

    // Page 3 (cursor from page 2): the final row. A short page (1 < 2) → no more cursor.
    let p3 = page(json!({ "cursor": c2 }));
    assert_eq!(p3.body["rows"], json!([{ "name": "e", "rank": 50 }]));
    assert_eq!(p3.body["cursor"], json!(null), "last page has no cursor");

    // A tampered cursor is rejected at the boundary (400), never fed to the query.
    let bad = page(json!({ "cursor": "deadbeef.00" }));
    assert_eq!(bad.status, 400, "{:?}", bad.body);
    assert_eq!(bad.body["error"]["code"], json!("bad_cursor"));
}

/// A to-**many** nested shape array (`items { … }`, L1) returns a JSON array of
/// sub-objects end-to-end: codegen aggregates the child rows into an `items[]` JSON-array
/// column (correlated subquery + SQLite `json_group_array`), the live SELECT returns it as
/// a string, and the runtime parses it into a real JSON array — proven against a real
/// engine. Also asserts a parent with no children returns `[]`, and the child's soft-delete
/// tombstone is respected (a deleted item is excluded from the array).
#[test]
fn nested_to_many_query_returns_json_array() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Order { total: int, items: OrderItem[] }
        @sort(id asc)
        @soft_delete(deleted_at)
        OrderItem { order: Order, sku: text, qty: int, deleted_at: timestamp? }
        shape OrderCard from Order { total, items { sku, qty } }
        query order_by_id(id) -> OrderCard;
        query orders() -> OrderCard[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `order` (`id`, `total`) VALUES ('o1', 500), ('o2', 0);
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`) VALUES
                ('i1', 'o1', 'ABC', 2), ('i2', 'o1', 'XYZ', 5);
            -- a soft-deleted item on o1 must be excluded from the array.
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`, `deleted_at`)
                VALUES ('i3', 'o1', 'GONE', 9, '2020-01-01 00:00:00');
            "#,
        )
        .expect("seed");

    // `get`: the child rows ride back nested under `items`, not as a flat string column.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "total": 500,
            "items": [{ "sku": "ABC", "qty": 2 }, { "sku": "XYZ", "qty": 5 }]
        }),
        "soft-deleted item excluded, remaining children nested"
    );

    // `list`: o2 has no items → an empty array (not null, not a missing field).
    let listed = call(&c, &backend, "POST", "/q/orders", json!({}), json!({}));
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([
            { "total": 500, "items": [{ "sku": "ABC", "qty": 2 }, { "sku": "XYZ", "qty": 5 }] },
            { "total": 0, "items": [] }
        ])
    );
}

/// The flagship **self-referential** to-many (`User.invited_users`, L1): a User joined to
/// itself under a distinct subquery alias. Proven end-to-end against a real engine — the
/// correlated subquery's `s<n>_user` alias never collides with the outer `user` row, so a
/// user's invitees nest correctly.
#[test]
fn nested_self_referential_to_many_returns_json_array() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        User {
          name: text
          invited_by: User?
          invited_users: User[] (User.invited_by)
        }
        shape UserCard from User { name, invited_users { name } }
        query user_by_id(id) -> UserCard;
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `invited_by_id`) VALUES
                ('u1', 'Ada', NULL), ('u2', 'Bob', 'u1'), ('u3', 'Cy', 'u1');
            "#,
        )
        .expect("seed");

    // Ada (u1) invited Bob + Cy: both nest under `invited_users`.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/user_by_id",
        json!({ "id": "u1" }),
        json!({}),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "name": "Ada", "invited_users": [{ "name": "Bob" }, { "name": "Cy" }] })
    );

    // Bob invited no one → an empty array.
    let leaf = call(
        &c,
        &backend,
        "POST",
        "/q/user_by_id",
        json!({ "id": "u2" }),
        json!({}),
    );
    assert_eq!(leaf.body, json!({ "name": "Bob", "invited_users": [] }));
}
