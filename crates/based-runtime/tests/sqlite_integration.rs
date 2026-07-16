//! End-to-end integration against a **real** engine (SQLite), no mock.
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
//! joins, `LIMIT`, positional `?`). The **DDL** is also generated for SQLite —
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
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
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
/// SQLite DDL (`based gen sql` with `Dialect::Sqlite`), then insert a couple of rows.
/// Running the real DDL — not a hand-shaped copy — means this test now exercises the whole
/// `based gen sql` artifact end to end: the DDL creates the schema the DML then reads/writes.
async fn seeded_backend(c: &Compiled) -> SqliteBackend {
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("generated SQLite DDL failed to execute: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`, `slug`) VALUES ('org-1', 'Acme', 'acme');
            INSERT INTO `user` (`id`, `email`, `name`) VALUES ('user-1', 'a@x.com', 'Ada');
            INSERT INTO `order` (`id`, `org_id`, `placed_by_id`, `status`, `total`)
                VALUES ('order-1', 'org-1', 'user-1', 'paid', '500.00');
            "#,
        )
        .await
        .expect("seed fixtures");
    backend
}

/// Run one request through the real dispatch core against the live backend — the exact
/// path `based serve` uses, minus the socket (dispatch checks its own connection out).
async fn call(
    compiled: &Compiled,
    backend: &SqliteBackend,
    method: &str,
    path: &str,
    args: serde_json::Value,
    ctx: serde_json::Value,
) -> based_runtime::WireResponse {
    let mut ids = SeqIdGen::default();
    dispatch(
        compiled,
        backend,
        "",
        &mut ids,
        &NoStore,
        &Guards::new(),
        method,
        path,
        args,
        ctx,
        None,
    )
    .await
}

#[tokio::test]
async fn get_query_runs_against_real_sqlite() {
    // `order_by_id` is a `get`: it joins order → user + org and projects the OrderCard
    // shape. This is the verbatim lowered SELECT executed against a live SQLite row.
    let c = commerce();
    let backend = seeded_backend(&c).await;
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "order-1" }),
        // Order is `@scope`d: even a keyed `get` is org-scoped, so `$ctx.org` is
        // required. order-1 belongs to org-1, so it's visible to this caller.
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "paid", "total": "500.00", "buyer": "Ada", "org": "Acme" })
    );
}

#[tokio::test]
async fn get_query_misses_return_null() {
    // A `get` on an absent key is `Option<T>` → JSON null (the envelope, realized by a
    // real empty result set, not a canned one).
    let c = commerce();
    let backend = seeded_backend(&c).await;
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "nope" }),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!(null));
}

#[tokio::test]
async fn ctx_scoped_list_query_binds_context() {
    // `my_org_orders` reads `$ctx.org` — the server supplies it out of band, and the
    // runtime binds it positionally into the WHERE. A `list` shapes as a JSON array.
    let c = commerce();
    let backend = seeded_backend(&c).await;
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!([{ "status": "paid", "total": "500.00", "buyer": "Ada", "org": "Acme" }])
    );

    // A different org sees none of org-1's rows — the injected scope predicate is real.
    let empty = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-other" }),
    )
    .await;
    assert_eq!(empty.body, json!([]));
}

#[tokio::test]
async fn mutation_writes_then_reselects_declared_shape() {
    // `place_order` creates an Order (engine-generated id) and reads it back in its
    // declared OrderCard shape, all under one transaction — the full write path
    // against a real engine: INSERT executes, the re-select joins and projects.
    let c = commerce();
    let backend = seeded_backend(&c).await;
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/place_order",
        // `org` is `@scope`-managed on create: supplied via `$ctx`, auto-set on the
        // INSERT — never a body arg. The re-select projects `org.name` = "Acme" (org-1).
        json!({ "buyer": "user-1", "total": "99.00" }),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    // The response is the created row in its declared shape (status defaults to 'pending').
    assert_eq!(
        resp.body,
        json!({ "status": "pending", "total": "99.00", "buyer": "Ada", "org": "Acme" })
    );

    // The write actually committed: the new order is now visible to a read.
    let listed = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-1" }),
    )
    .await;
    let rows = listed.body.as_array().expect("list");
    assert_eq!(
        rows.len(),
        2,
        "the created order is now readable: {:?}",
        rows
    );
}

#[tokio::test]
async fn bad_arg_is_a_400_before_sql() {
    // A mistyped arg is a boundary error caught before any SQL touches SQLite. `total` is a
    // `decimal` — its wire form is a JSON string, so a bare number is the wrong shape.
    let c = commerce();
    let backend = seeded_backend(&c).await;
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/place_order",
        json!({ "buyer": "user-1", "total": 4200 }),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 400, "{:?}", resp.body);
    assert_eq!(resp.body["error"]["code"], json!("bad_arg"));
}

#[tokio::test]
async fn backend_ping_succeeds_on_a_live_db() {
    // The readiness seam works against a real engine: `SELECT 1` round-trips.
    let c = commerce();
    assert!(seeded_backend(&c).await.ping().await.is_ok());
}

/// An `update` mutation reads its row back in the **full declared shape**, not a bare
/// `{ id }`, keyed off the write's own `where` and run inside the same transaction
/// (read-your-writes) — proven against a real engine. The shape includes a nested to-one
/// sub-object (`placed_by { name }`), so the re-select exercises a relation join too.
#[tokio::test]
async fn update_mutation_reselects_full_declared_shape_end_to_end() {
    let c = compile_sqlite(
        r#"
        User { name: text }
        @updated(updated_at)
        Order { updated_at: timestamp, placed_by: User, status: text, total: int }
        shape OrderCard from Order { status, total, placed_by { name } }
        mutation set_status(id: Id, status: text) -> OrderCard {
          update Order where (id = $id) { status = $status };
        }
        query order_by_id(id) -> OrderCard;
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
            INSERT INTO `user` (`id`, `name`) VALUES ('u1', 'Ada');
            INSERT INTO `order` (`id`, `updated_at`, `placed_by_id`, `status`, `total`)
                VALUES ('o1', '2020-01-01 00:00:00', 'u1', 'pending', 99);
            "#,
        )
        .await
        .expect("seed");

    // Update the status; the response is the *updated* row in its full declared shape
    // (new status, the unchanged total, and the nested buyer) — not `{ id }`.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/set_status",
        json!({ "id": "o1", "status": "shipped" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "shipped", "total": 99, "placed_by": { "name": "Ada" } }),
        "read-your-writes: the re-select sees the new status under the same tx"
    );

    // The write committed: a fresh read sees the new status too.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
    )
    .await;
    assert_eq!(
        got.body,
        json!({ "status": "shipped", "total": 99, "placed_by": { "name": "Ada" } })
    );
}

/// An update whose `where` matches no row — a wrong id, or a cross-tenant id the scope
/// filter excludes — is a 404 `not_found` with nothing written, never a `200` with a
/// null body the typed client cannot decode.
#[tokio::test]
async fn zero_row_update_is_404_and_writes_nothing_end_to_end() {
    let c = compile_sqlite(
        r#"
        Org { name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        @updated(updated_at)
        Order { updated_at: timestamp, org: Org, status: text }
        shape OrderCard from Order { status }
        mutation set_status(id: Id, status: text) -> OrderCard scoped Tenant {
          update Order where (id = $id) { status = $status };
        }
        query order_by_id(id) -> OrderCard scoped Tenant;
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
            INSERT INTO `org` (`id`, `name`) VALUES ('org-a', 'A'), ('org-b', 'B');
            INSERT INTO `order` (`id`, `updated_at`, `org_id`, `status`)
                VALUES ('o1', '2020-01-01 00:00:00', 'org-a', 'pending');
            "#,
        )
        .await
        .expect("seed");

    // Another tenant names org-a's row: the scope filter makes the UPDATE match nothing.
    let cross = call(
        &c,
        &backend,
        "POST",
        "/m/set_status",
        json!({ "id": "o1", "status": "shipped" }),
        json!({ "org": "org-b" }),
    )
    .await;
    assert_eq!(cross.status, 404, "{:?}", cross.body);
    assert_eq!(cross.body["error"]["code"], "not_found");

    // A genuinely absent id under the owning tenant: same 404.
    let absent = call(
        &c,
        &backend,
        "POST",
        "/m/set_status",
        json!({ "id": "no-such", "status": "shipped" }),
        json!({ "org": "org-a" }),
    )
    .await;
    assert_eq!(absent.status, 404, "{:?}", absent.body);
    assert_eq!(absent.body["error"]["code"], "not_found");

    // Nothing was written: the owner still reads the original status.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({ "org": "org-a" }),
    )
    .await;
    assert_eq!(got.body, json!({ "status": "pending" }));
}

/// A scoped child reached **only** through a nested shape sub-object is confined by its
/// `@scope`, so a cross-scope nested read can't leak rows the caller's `$ctx` excludes —
/// mirroring D34's Ticket→Contact but through nests. Here the parent `Order` is scoped on
/// `Tenant` (org) and its nested children are scoped on a *divergent* axis (`Region`): a
/// to-one `contact { name }` and a to-many `items { sku }`. Against a live engine, an
/// out-of-scope contact's name reads back NULL (not the real name) and an out-of-scope
/// line item is absent from the array — proven, not compile-only.
#[tokio::test]
async fn nest_reached_scoped_child_is_confined_cross_tenant() {
    let c = compile_sqlite(
        r#"
        Org { name: text }
        Region { name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        @sort(id asc)
        Order { org: Org, contact: Contact?, total: int, items: LineItem[] }
        @scope Region
        @sort(id asc)
        Contact { region: Region, name: text }
        @scope Region
        @sort(id asc)
        LineItem { order: Order, region: Region, sku: text }
        shape OrderCard from Order { total, contact { name }, items { sku } }
        query order_by_id(id) -> OrderCard scoped Tenant, Region;
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
            INSERT INTO `org` (`id`, `name`) VALUES ('org-1', 'Acme');
            INSERT INTO `region` (`id`, `name`) VALUES ('r1', 'North'), ('r2', 'South');
            INSERT INTO `contact` (`id`, `region_id`, `name`)
                VALUES ('c1', 'r1', 'InRegion'), ('c2', 'r2', 'OutRegion');
            INSERT INTO `order` (`id`, `org_id`, `contact_id`, `total`)
                VALUES ('o1', 'org-1', 'c1', 100), ('o2', 'org-1', 'c2', 200);
            INSERT INTO `line_item` (`id`, `order_id`, `region_id`, `sku`)
                VALUES ('li1', 'o1', 'r1', 'IN'), ('li2', 'o1', 'r2', 'OUT');
            "#,
        )
        .await
        .expect("seed");

    let ctx = json!({ "org": "org-1", "region": "r1" });

    // o1: contact c1 is in-region → its name reads back; item li1 is in-region, li2 is
    // out-of-region → only li1 survives the correlated subquery's scope predicate.
    let o1 = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        ctx.clone(),
    )
    .await;
    assert_eq!(o1.status, 200, "{:?}", o1.body);
    assert_eq!(
        o1.body,
        json!({ "total": 100, "contact": { "name": "InRegion" }, "items": [{ "sku": "IN" }] })
    );

    // o2: contact c2 is out-of-region → the nest join finds no in-scope row, so the
    // whole nest reads back as JSON null (an absent optional to-one) instead of
    // leaking "OutRegion"; o2 has no in-region items.
    let o2 = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o2" }),
        ctx,
    )
    .await;
    assert_eq!(o2.status, 200, "{:?}", o2.body);
    assert_eq!(
        o2.body,
        json!({ "total": 200, "contact": null, "items": [] }),
        "cross-scope nested read must not leak the out-of-region contact"
    );
}

/// End-to-end enum round-trip against a live engine: a string enum with a name≠value
/// variant (`paid = "PAID"`) and an int enum (`Priority`). A create assigns both by name;
/// the response shape carries their *wire* values (`"PAID"`, `2`); a filter on the string
/// enum and an *ordered* filter on the int enum each return the row; and the DB CHECK
/// rejects an out-of-range value directly inserted — proving the constraint is live.
#[tokio::test]
async fn enum_round_trip_string_and_int_end_to_end() {
    let c = compile_sqlite(
        r#"
        enum Status { pending, paid = "PAID", shipped }
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { status: Status (default pending), priority: Priority, title: text }
        shape TicketRow from Ticket { status, priority, title }
        mutation open_ticket(title: text) -> TicketRow {
          create Ticket { title = $title, priority = high, status = paid };
        }
        query urgent() -> TicketRow[] { list Ticket where (priority >= medium) order (title); }
        query by_paid() -> TicketRow[] { list Ticket where (status = paid) order (title); }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    // Create: `status = paid` and `priority = high` are assigned by name; the response
    // carries their wire values ("PAID" for the renamed string variant, 2 for the int).
    let created = call(
        &c,
        &backend,
        "POST",
        "/m/open_ticket",
        json!({ "title": "server down" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(
        created.body,
        json!({ "status": "PAID", "priority": 2, "title": "server down" }),
        "the shaped value carries the enum wire representation"
    );

    // Ordered filter on the int enum (`priority >= medium`) returns the row live.
    let urgent = call(&c, &backend, "POST", "/q/urgent", json!({}), json!({})).await;
    assert_eq!(urgent.status, 200, "{:?}", urgent.body);
    assert_eq!(
        urgent.body,
        json!([{ "status": "PAID", "priority": 2, "title": "server down" }])
    );

    // Filter on the string enum by name (`status = paid` → 'PAID') returns the row.
    let paid = call(&c, &backend, "POST", "/q/by_paid", json!({}), json!({})).await;
    assert_eq!(
        paid.body,
        json!([{ "status": "PAID", "priority": 2, "title": "server down" }])
    );

    // The DB CHECK rejects an out-of-range value inserted directly (defense in depth).
    let bad_int = backend.execute_batch(
        "INSERT INTO `ticket` (`id`, `status`, `priority`, `title`) VALUES ('x', 'PAID', 99, 't');",
    ).await;
    assert!(bad_int.is_err(), "int-enum CHECK must reject 99");
    let bad_str = backend.execute_batch(
        "INSERT INTO `ticket` (`id`, `status`, `priority`, `title`) VALUES ('y', 'bogus', 0, 't');",
    ).await;
    assert!(bad_str.is_err(), "string-enum CHECK must reject 'bogus'");
}

#[tokio::test]
async fn decimal_and_float_round_trip_end_to_end() {
    let c = compile_sqlite(
        r#"
        Ledger { name: text, price: decimal(12, 2), score: float }
        shape LedgerRow from Ledger { name, price, score }
        mutation add_entry(name: text, price: decimal(12, 2), score: float) -> LedgerRow {
          create Ledger { name = $name, price = $price, score = $score };
        }
        query pricey(min: decimal(12, 2) >= price) -> LedgerRow[]
          unindexed(unsafe) order (name);
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    // A seeded row whose exact decimal (with a trailing zero) must survive the read.
    backend
        .execute_batch(
            "INSERT INTO `ledger` (`id`, `name`, `price`, `score`) VALUES ('seed', 'cheap', '0.10', 0.25);",
        ).await
        .expect("seed");

    // Create: a decimal is sent (and returned) as its exact string, a float as a number.
    let big = call(
        &c,
        &backend,
        "POST",
        "/m/add_entry",
        json!({ "name": "pricey", "price": "19.99", "score": 1.5 }),
        json!({}),
    )
    .await;
    assert_eq!(big.status, 200, "{:?}", big.body);
    assert_eq!(
        big.body,
        json!({ "name": "pricey", "price": "19.99", "score": 1.5 }),
        "create re-selects the row with the decimal exact + float as a number"
    );

    // An ordered comparison (`price >= 10.00`) filters correctly; the seeded `0.10` is
    // excluded, the created `19.99` kept — and both read back byte-exact.
    let filtered = call(
        &c,
        &backend,
        "POST",
        "/q/pricey",
        json!({ "min": "10.00" }),
        json!({}),
    )
    .await;
    assert_eq!(filtered.status, 200, "{:?}", filtered.body);
    assert_eq!(
        filtered.body,
        json!([{ "name": "pricey", "price": "19.99", "score": 1.5 }])
    );
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

#[tokio::test]
async fn joined_scope_hides_cross_scope_row_end_to_end() {
    // Proven against a real engine: a query on the *unscoped* `Ticket` reaches the
    // org-*scoped* `Contact` through `raised_by`. Codegen injects `Contact`'s `@scope`
    // into the join `ON` (`contact.org_id = :ctx_org`), so a contact belonging to another
    // org is invisible across the join.
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

/// A to-one nested shape sub-object (`placed_by { name, email }`) returns a nested
/// JSON object end-to-end: the codegen-prefixed columns (`placed_by.name`, …) come back
/// from the live SELECT and the runtime reassembles them into a sub-object — proven
/// against a real engine, not compile-verified. Self-contained (no commerce schema) so
/// the nesting is the only variable.
#[tokio::test]
async fn nested_to_one_query_returns_nested_json() {
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
        .await
        .unwrap_or_else(|e| panic!("generated DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `email`) VALUES ('u1', 'Ada', 'a@x.com');
            INSERT INTO `order` (`id`, `placed_by_id`, `total`) VALUES ('o1', 'u1', 500);
            "#,
        )
        .await
        .expect("seed");

    // `get`: the nested object rides back under `placed_by`, not as flat `placed_by.*`.
    let mut ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &backend,
        "",
        &mut ids,
        &NoStore,
        &Guards::new(),
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "total": 500, "placed_by": { "name": "Ada", "email": "a@x.com" } })
    );

    // `list`: every row reassembles independently.
    let listed = dispatch(
        &c,
        &backend,
        "",
        &mut ids,
        &NoStore,
        &Guards::new(),
        "POST",
        "/q/orders",
        json!({}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([{ "total": 500, "placed_by": { "name": "Ada", "email": "a@x.com" } }])
    );
}

/// An optional to-one nest whose row is absent comes back as JSON `null` — never an
/// object of nulls (which a typed client cannot decode into `Option<Shape>`) — while a
/// present row nests normally and sheds the internal presence probe. Proven at both
/// levels: a top-level nest (flat-column reassembly) and a nest inside a to-many JSON
/// aggregate (SQL-built objects).
#[tokio::test]
async fn absent_optional_to_one_nest_is_json_null() {
    let src = r#"
        User { name: text, email: text }
        @sort(id asc)
        Order {
          placed_by:    User
          fulfilled_by: User?
          total:        int
          items:        Item[] (Item.order)
        }
        @sort(id asc)
        Item { order: Order, checker: User?, qty: int }
        shape OrderCard from Order {
          total
          placed_by { name }
          fulfilled_by { name, email }
          items { qty, checker { name } }
        }
        query order_by_id(id) -> OrderCard;
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
    backend.execute_batch(&ddl).await.unwrap_or_else(|e| {
        panic!(
            "generated DDL failed: {e:?}
{ddl}"
        )
    });
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `email`) VALUES ('u1', 'Ada', 'a@x.com');
            INSERT INTO `order` (`id`, `placed_by_id`, `fulfilled_by_id`, `total`)
                VALUES ('o1', 'u1', NULL, 500), ('o2', 'u1', 'u1', 700);
            INSERT INTO `item` (`id`, `order_id`, `checker_id`, `qty`)
                VALUES ('i1', 'o1', NULL, 3), ('i2', 'o1', 'u1', 4);
            "#,
        )
        .await
        .expect("seed");

    let mut ids = SeqIdGen::default();
    let absent = dispatch(
        &c,
        &backend,
        "",
        &mut ids,
        &NoStore,
        &Guards::new(),
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(absent.status, 200, "{:?}", absent.body);
    assert_eq!(
        absent.body,
        json!({
            "total": 500,
            "placed_by": { "name": "Ada" },
            "fulfilled_by": null,
            "items": [
                { "qty": 3, "checker": null },
                { "qty": 4, "checker": { "name": "Ada" } },
            ],
        })
    );

    let present = dispatch(
        &c,
        &backend,
        "",
        &mut ids,
        &NoStore,
        &Guards::new(),
        "POST",
        "/q/order_by_id",
        json!({ "id": "o2" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(present.status, 200, "{:?}", present.body);
    assert_eq!(
        present.body["fulfilled_by"],
        json!({ "name": "Ada", "email": "a@x.com" }),
        "a matched optional nest sheds the presence probe"
    );
}

/// Keyset-cursor pagination, proven against a real engine: paging a `page (2)`
/// keyset query walks the whole set exactly once — each page returns the next window and
/// an opaque cursor, the final short page returns a `null` cursor, and the cursor works
/// even though the sort basis (`rank`, `id`) is not projected (the runtime strips the
/// hidden `__keyset_*` columns from the response). A tampered cursor is a 400.
#[tokio::test]
async fn keyset_pagination_walks_the_set_end_to_end() {
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
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `item` (`id`, `name`, `rank`) VALUES
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30),
                ('i4', 'd', 40), ('i5', 'e', 50);
            "#,
        )
        .await
        .expect("seed");

    let page = |args: serde_json::Value| call(&c, &backend, "POST", "/q/items", args, json!({}));

    // Page 1 (no cursor): the two lowest-ranked rows + a "more" cursor (a full page).
    // Rows carry only the projected `{name, rank}` — the hidden sort-key columns are
    // stripped even though they drive the cursor.
    let p1 = page(json!({})).await;
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
    let p2 = page(json!({ "cursor": c1 })).await;
    assert_eq!(
        p2.body["rows"],
        json!([{ "name": "c", "rank": 30 }, { "name": "d", "rank": 40 }])
    );
    let c2 = p2.body["cursor"]
        .as_str()
        .expect("page 2 cursor")
        .to_string();

    // Page 3 (cursor from page 2): the final row. A short page (1 < 2) → no more cursor.
    let p3 = page(json!({ "cursor": c2 })).await;
    assert_eq!(p3.body["rows"], json!([{ "name": "e", "rank": 50 }]));
    assert_eq!(p3.body["cursor"], json!(null), "last page has no cursor");

    // A tampered cursor is rejected at the boundary (400), never fed to the query.
    let bad = page(json!({ "cursor": "deadbeef.00" })).await;
    assert_eq!(bad.status, 400, "{:?}", bad.body);
    assert_eq!(bad.body["error"]["code"], json!("bad_cursor"));
}

/// A to-**many** nested shape array (`items { … }`) returns a JSON array of
/// sub-objects end-to-end: codegen aggregates the child rows into an `items[]` JSON-array
/// column (correlated subquery + SQLite `json_group_array`), the live SELECT returns it as
/// a string, and the runtime parses it into a real JSON array — proven against a real
/// engine. Also asserts a parent with no children returns `[]`, and the child's soft-delete
/// tombstone is respected (a deleted item is excluded from the array).
#[tokio::test]
async fn nested_to_many_query_returns_json_array() {
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
        .await
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
        .await
        .expect("seed");

    // `get`: the child rows ride back nested under `items`, not as a flat string column.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
    )
    .await;
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
    let listed = call(&c, &backend, "POST", "/q/orders", json!({}), json!({})).await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([
            { "total": 500, "items": [{ "sku": "ABC", "qty": 2 }, { "sku": "XYZ", "qty": 5 }] },
            { "total": 0, "items": [] }
        ])
    );
}

/// The flagship **self-referential** to-many (`User.invited_users`): a User joined to
/// itself under a distinct subquery alias. Proven end-to-end against a real engine — the
/// correlated subquery's `s<n>_user` alias never collides with the outer `user` row, so a
/// user's invitees nest correctly.
#[tokio::test]
async fn nested_self_referential_to_many_returns_json_array() {
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
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `invited_by_id`) VALUES
                ('u1', 'Ada', NULL), ('u2', 'Bob', 'u1'), ('u3', 'Cy', 'u1');
            "#,
        )
        .await
        .expect("seed");

    // Ada (u1) invited Bob + Cy: both nest under `invited_users`.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/user_by_id",
        json!({ "id": "u1" }),
        json!({}),
    )
    .await;
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
    )
    .await;
    assert_eq!(leaf.body, json!({ "name": "Bob", "invited_users": [] }));
}

/// A to-many nested array rides back in **sort-cascade order**, proven live: children
/// seeded out of order come back ordered by the child model's `@sort` (`comments`), and
/// a relation `@sort` on the edge overrides the child model's own (`pins`). The ORDER BY
/// lives inside the JSON aggregate, so the outer query's shape is untouched.
#[tokio::test]
async fn nested_to_many_rows_ride_in_sort_cascade_order() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Ticket {
          subject: text
          comments: Comment[]
          pins: Pin[] @sort(rank desc)
        }
        @sort(pos asc)
        Comment { ticket: Ticket, pos: int, body: text }
        @sort(rank asc)
        Pin { ticket: Ticket, rank: int, label: text }
        shape TicketDetail from Ticket { subject, comments { body }, pins { label } }
        query ticket_by_id(id) -> TicketDetail;
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
            INSERT INTO `ticket` (`id`, `subject`) VALUES ('t1', 'printer on fire');
            -- seeded out of `pos` order: the array order must come from the sort, not insertion.
            INSERT INTO `comment` (`id`, `ticket_id`, `pos`, `body`) VALUES
                ('c3', 't1', 3, 'third'), ('c1', 't1', 1, 'first'), ('c2', 't1', 2, 'second');
            INSERT INTO `pin` (`id`, `ticket_id`, `rank`, `label`) VALUES
                ('p1', 't1', 1, 'low'), ('p3', 't1', 3, 'top'), ('p2', 't1', 2, 'mid');
            "#,
        )
        .await
        .expect("seed");

    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/ticket_by_id",
        json!({ "id": "t1" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "subject": "printer on fire",
            // child model `@sort(pos asc)` — the model tier of the cascade.
            "comments": [{ "body": "first" }, { "body": "second" }, { "body": "third" }],
            // relation `@sort(rank desc)` overrides Pin's model `@sort(rank asc)`.
            "pins": [{ "label": "top" }, { "label": "mid" }, { "label": "low" }]
        })
    );
}

/// A **named-shape** nest (`placed_by -> UserRef`, `items -> ItemRow`) returns the same
/// nested JSON an inline nest does, end-to-end against a real engine: the reference is a
/// pure body expansion (same SQL, same `nest_row`/array reassembly), the payoff being the
/// shared nominal type on the client. Covers to-one and to-many refs in one shape, with a
/// soft-deleted child excluded exactly as the nest context dictates.
#[tokio::test]
async fn named_shape_nest_returns_nested_json() {
    let c = compile_sqlite(
        r#"
        User { name: text, email: text }
        @sort(id asc)
        Order { placed_by: User, total: int, items: OrderItem[] }
        @sort(id asc)
        @soft_delete(deleted_at)
        OrderItem { order: Order, sku: text, qty: int, deleted_at: timestamp? }
        shape UserRef from User { name, email }
        shape ItemRow from OrderItem { sku, qty }
        shape OrderDetail from Order {
          total
          placed_by -> UserRef
          items -> ItemRow
        }
        query order_detail(id) -> OrderDetail;
        query order_details() -> OrderDetail[];
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
            INSERT INTO `user` (`id`, `name`, `email`) VALUES ('u1', 'Ada', 'a@x.com');
            INSERT INTO `order` (`id`, `placed_by_id`, `total`) VALUES
                ('o1', 'u1', 500), ('o2', 'u1', 0);
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`) VALUES
                ('i1', 'o1', 'ABC', 2);
            -- a soft-deleted item must be excluded, exactly as with an inline nest.
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`, `deleted_at`)
                VALUES ('i2', 'o1', 'GONE', 9, '2020-01-01 00:00:00');
            "#,
        )
        .await
        .expect("seed");

    // `get`: buyer nests as the named `UserRef` projection, items as `ItemRow` elements.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_detail",
        json!({ "id": "o1" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "total": 500,
            "placed_by": { "name": "Ada", "email": "a@x.com" },
            "items": [{ "sku": "ABC", "qty": 2 }]
        })
    );

    // `list`: every row reassembles; a childless order yields `[]`.
    let listed = call(
        &c,
        &backend,
        "POST",
        "/q/order_details",
        json!({}),
        json!({}),
    )
    .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([
            {
                "total": 500,
                "placed_by": { "name": "Ada", "email": "a@x.com" },
                "items": [{ "sku": "ABC", "qty": 2 }]
            },
            {
                "total": 0,
                "placed_by": { "name": "Ada", "email": "a@x.com" },
                "items": []
            }
        ])
    );
}

/// The commerce example's `order_detail` (its `OrderDetail` nests `placed_by ->
/// UserRef`) runs live: the worked example's named-shape reference is executable,
/// not just documentation.
#[tokio::test]
async fn commerce_order_detail_nests_named_user_ref() {
    let c = commerce();
    let backend = seeded_backend(&c).await;
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_detail",
        json!({ "id": "order-1" }),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "status": "paid",
            "total": "500.00",
            "placed_by": { "name": "Ada", "email": "a@x.com" }
        })
    );
}

#[tokio::test]
async fn enum_variant_filter_and_check_constraint_end_to_end() {
    // A self-contained enum schema: a `where status = <variant>` filter (lowered to a
    // string literal) executed live, plus proof the DB CHECK rejects a non-variant value.
    let c = compile_sqlite(
        r#"
        enum Status { pending, paid, shipped }
        Item {
          status: Status (default pending)
          name:   text
        }
        shape ItemRow from Item { status, name }
        query paid_items() -> ItemRow[] { list Item where (status = paid) order (name); }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .expect("generated DDL executes");
    backend
        .execute_batch(
            r#"
            INSERT INTO `item` (`id`, `status`, `name`) VALUES ('i1', 'paid', 'A');
            INSERT INTO `item` (`id`, `status`, `name`) VALUES ('i2', 'pending', 'B');
            INSERT INTO `item` (`id`, `status`, `name`) VALUES ('i3', 'paid', 'C');
            "#,
        )
        .await
        .expect("seed enum rows");

    // The variant filter runs live and returns only the two `paid` rows, with the enum
    // value round-tripping as its wire string.
    let resp = call(&c, &backend, "POST", "/q/paid_items", json!({}), json!({})).await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!([
            { "status": "paid", "name": "A" },
            { "status": "paid", "name": "C" }
        ])
    );

    // The generated CHECK constraint rejects a value outside the enum's variants.
    let bad = backend
        .execute_batch("INSERT INTO `item` (`id`, `status`, `name`) VALUES ('x', 'bogus', 'X');")
        .await;
    assert!(
        bad.is_err(),
        "DB should reject a non-variant enum value via the CHECK constraint"
    );
}

/// A host guard (auth.md Handle 3) that genuinely **reads the live database** before
/// deciding: closing an order is allowed while its row is still open; once the write
/// lands, the same call is denied because the guard's own SELECT sees the new state.
/// The whole pass — guard read, denial, and the guarded write — runs through the real
/// dispatch core against live SQLite.
#[tokio::test]
async fn guard_reads_the_live_database_before_the_write() {
    use based_runtime::{fetch_all, GuardVerdict, SqlValue};
    use std::sync::Arc;

    const SCHEMA: &str = r#"
        Order { status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation close_order(id) -> OrderCard guard order_still_open {
            update Order where (id = $id) { status = "closed" };
        }
    "#;
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(!diags
        .iter()
        .any(|d| d.severity == based_diagnostics::Severity::Error));
    let c = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);

    let backend = Arc::new(SqliteBackend::in_memory().expect("open in-memory sqlite"));
    backend
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("generated DDL");
    backend
        .execute_batch("INSERT INTO `order` (`id`, `status`, `total`) VALUES ('o-1', 'open', 9);")
        .await
        .expect("seed");

    // The guard owns its own resources: it captures the backend and runs its own
    // SELECT. It checks out before the mutation does, so the single pooled
    // connection is free during the read.
    let guard_backend = Arc::clone(&backend);
    let guards = Guards::new().register("order_still_open", move |req| {
        let backend = Arc::clone(&guard_backend);
        async move {
            let id = req.args["id"].as_str().unwrap_or_default().to_string();
            let mut conn = match backend.checkout("").await {
                Ok(conn) => conn,
                // Fail closed: a guard that cannot decide denies.
                Err(_) => return GuardVerdict::deny("cannot verify order state"),
            };
            let rows = fetch_all(conn.fetch(
                "SELECT `status` FROM `order` WHERE `id` = ?",
                &[SqlValue::Text(id)],
            ))
            .await;
            match rows {
                Ok(rows)
                    if rows
                        .first()
                        .and_then(|r| r.get("status"))
                        .and_then(|v| v.as_str())
                        == Some("open") =>
                {
                    GuardVerdict::Allow
                }
                Ok(_) => GuardVerdict::deny("order is not open"),
                Err(_) => GuardVerdict::deny("cannot verify order state"),
            }
        }
    });

    let run = |args: serde_json::Value| {
        let c = &c;
        let backend = Arc::clone(&backend);
        let guards = &guards;
        async move {
            let mut ids = SeqIdGen::default();
            dispatch(
                c,
                &*backend,
                "",
                &mut ids,
                &NoStore,
                guards,
                "POST",
                "/m/close_order",
                args,
                json!({}),
                None,
            )
            .await
        }
    };

    // Open row → the guard's SELECT sees 'open' → allowed; the write lands and the
    // declared-shape re-select returns the closed row.
    let first = run(json!({ "id": "o-1" })).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({ "status": "closed", "total": 9 }));

    // Same call again: the guard's SELECT now sees 'closed' → denied, before any write.
    let second = run(json!({ "id": "o-1" })).await;
    assert_eq!(second.status, 403);
    assert_eq!(second.body["error"]["code"], "guard_denied");
    assert_eq!(second.body["error"]["message"], "order is not open");
}
