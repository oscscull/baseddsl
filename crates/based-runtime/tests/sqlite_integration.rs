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
    let ids = SeqIdGen::default();
    dispatch(
        compiled,
        backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
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
    assert_eq!(rows.len(), 2, "the created order is now readable: {rows:?}");
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
        User { id: Id, name: text }
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, placed_by: User, status: text, total: int }
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

/// An atomic update expression (`total = total + $delta`) is computed **server-side** in
/// one statement, not read-modify-write — proven against a real engine: the read-your-writes
/// re-select shows the arithmetic result, and two sequential adjustments compose off the
/// stored value (no lost update).
#[tokio::test]
async fn atomic_update_expression_computes_server_side_end_to_end() {
    let c = compile_sqlite(
        r#"
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation adjust_total(id: Id, delta: int) -> OrderCard {
          update Order where (id = $id) { total = total + $delta };
        }
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
            INSERT INTO `order` (`id`, `updated_at`, `status`, `total`)
                VALUES ('o1', '2020-01-01 00:00:00', 'pending', 100);
            "#,
        )
        .await
        .expect("seed");

    // First adjustment: 100 + 25 = 125, computed in the database.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/adjust_total",
        json!({ "id": "o1", "delta": 25 }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "pending", "total": 125 }),
        "read-your-writes: the re-select sees the server-computed sum"
    );

    // Second adjustment composes off the stored value: 125 - 5 = 120.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/adjust_total",
        json!({ "id": "o1", "delta": -5 }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!({ "status": "pending", "total": 120 }));
}

/// A keyless (`@no_id`) legacy table inserts + reads back end to end: no `id` column
/// or `PRIMARY KEY` in the DDL, the INSERT sets no id, and the create's declared-shape
/// re-select keys on the `(unique)` column the create set (not a generated id). A `get`
/// keys on the same unique field.
#[tokio::test]
async fn keyless_model_inserts_and_reads_back_by_unique_end_to_end() {
    let c = compile_sqlite(
        r#"
        @no_id("append-only audit log keyed by its natural source, no surrogate id")
        AuditEvent { source: text (unique), action: text }
        shape EventRow from AuditEvent { source, action }
        query event_by_source(source) -> EventRow;
        mutation record_event(source: text, action: text) -> EventRow {
          create AuditEvent { source = $source, action = $action };
        }
        "#,
    );
    // The DDL is keyless: no `id` column, no PRIMARY KEY.
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    assert!(
        !ddl.contains("PRIMARY KEY"),
        "keyless table has no PK:\n{ddl}"
    );
    assert!(
        !ddl.contains("`id`"),
        "keyless table has no id column:\n{ddl}"
    );

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    // Create: no generated id; the row reads back keyed on its unique `source`.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/record_event",
        json!({ "source": "svc-a", "action": "login" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!({ "source": "svc-a", "action": "login" }));

    // Get by the unique key returns the same row.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/event_by_source",
        json!({ "source": "svc-a" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!({ "source": "svc-a", "action": "login" }));
}

/// An update whose `where` matches no row — a wrong id, or a cross-tenant id the scope
/// filter excludes — is a 404 `not_found` with nothing written, never a `200` with a
/// null body the typed client cannot decode.
#[tokio::test]
async fn zero_row_update_is_404_and_writes_nothing_end_to_end() {
    let c = compile_sqlite(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, org: Org, status: text }
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
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        @sort(id asc)
        Order { id: Id, org: Org, contact: Contact?, total: int, items: LineItem[] }
        @scope Region
        @sort(id asc)
        Contact { id: Id, region: Region, name: text }
        @scope Region
        @sort(id asc)
        LineItem { id: Id, order: Order, region: Region, sku: text }
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
        Ticket { id: Id, status: Status (default pending), priority: Priority, title: text }
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

/// `in` value-list live: variants lower to their wire values, a `$param` element
/// binds at run time (its wire value on the wire), and rows outside the list are
/// excluded.
#[tokio::test]
async fn in_value_list_filters_end_to_end() {
    let c = compile_sqlite(
        r#"
        enum Status { pending, paid = "PAID", shipped }
        Ticket { id: Id, status: Status, title: text, @index(status) }
        shape TicketRow from Ticket { status, title }
        query active(extra: Status) -> TicketRow[] {
          list Ticket where (status in (pending, $extra)) order (title);
        }
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
            "INSERT INTO `ticket` (`id`, `status`, `title`) VALUES
               ('a', 'pending', 'first'),
               ('b', 'PAID', 'second'),
               ('c', 'shipped', 'third');",
        )
        .await
        .expect("seed");

    // `$extra` carries the wire value ("PAID"); `shipped` is outside the list.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/active",
        json!({ "extra": "PAID" }),
        json!({}),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(
        got.body,
        json!([
            { "status": "pending", "title": "first" },
            { "status": "PAID", "title": "second" }
        ])
    );
}

#[tokio::test]
async fn decimal_and_float_round_trip_end_to_end() {
    let c = compile_sqlite(
        r#"
        Ledger { id: Id, name: text, price: decimal(12, 2), score: float }
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

/// A real `GROUP BY` / `HAVING` aggregate query executes against SQLite: `count()`, `sum`
/// (int + decimal), `avg`, and `max` group per buyer, soft-deleted rows are excluded before
/// grouping, `having` filters groups, and `order` sorts them — all computed in the database
/// and decoded to the declared wire types. `sum`/`max` over a `decimal` is float-degraded
/// on SQLite (documented — decimal is TEXT affinity there; production dialects DECIMAL/
/// NUMERIC are exact), so the exact-value proofs use int columns.
#[tokio::test]
async fn aggregate_group_by_having_end_to_end() {
    let c = compile_sqlite(
        r#"
        Buyer { id: Id, name: text }
        @soft_delete(deleted_at)
        Order {
          id: Id
          deleted_at: timestamp?
          buyer:      Buyer
          total:      decimal(12, 2)
          qty:        int
        }
        shape BuyerStats from Order {
          buyer   = buyer
          orders  = count()
          revenue = sum(total)
          units   = sum(qty)
          avg_qty = avg(qty)
          top_qty = max(qty)
        }
        query buyer_stats() -> BuyerStats[] {
          list Order group by (buyer) having (units > 3) order (units desc);
        }
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
            "INSERT INTO `buyer` (`id`, `name`) VALUES ('b1', 'Ada'), ('b2', 'Bo');
             INSERT INTO `order` (`id`, `buyer_id`, `total`, `qty`, `deleted_at`) VALUES
               ('o1', 'b1', '100.00',  2, NULL),
               ('o2', 'b1',  '50.50',  3, NULL),
               ('o3', 'b1',  '99.99',  9, '2024-01-01 00:00:00'),
               ('o4', 'b2',  '10.00',  4, NULL);",
        )
        .await
        .expect("seed");

    let got = call(&c, &backend, "POST", "/q/buyer_stats", json!({}), json!({})).await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    // Only Ada's two live orders count (o3 is soft-deleted, so its qty 9 is excluded
    // *before* grouping). units = 2 + 3 = 5 for b1, 4 for b2 (both > 3, pass `having`);
    // `order (units desc)` puts b1 first. count = 2/1, avg_qty = 2.5/4.0, max qty = 3/4
    // (exact on int). revenue is the SQLite float-degraded decimal sum.
    assert_eq!(
        got.body,
        json!([
            {
                "buyer": "b1",
                "orders": 2,
                "revenue": "150.5",
                "units": 5,
                "avg_qty": 2.5,
                "top_qty": 3
            },
            {
                "buyer": "b2",
                "orders": 1,
                "revenue": "10.0",
                "units": 4,
                "avg_qty": 4.0,
                "top_qty": 4
            }
        ])
    );
}

/// Compile an in-line schema for SQLite (skip disk), mirroring `Compiled::load`.
fn compile_sqlite(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
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

/// A to-one nested shape sub-object (`placed_by { name, email }`) returns a nested
/// JSON object end-to-end: the codegen-prefixed columns (`placed_by.name`, …) come back
/// from the live SELECT and the runtime reassembles them into a sub-object — proven
/// against a real engine, not compile-verified. Self-contained (no commerce schema) so
/// the nesting is the only variable.
#[tokio::test]
async fn nested_to_one_query_returns_nested_json() {
    let src = r#"
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, fulfilled_by: User?, total: int }
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
    let ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
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
        &ids,
        &NoStore,
        &Guards::new(),
        None,
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
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order {
          id: Id
          placed_by:    User
          fulfilled_by: User?
          total:        int
          items:        Item[] (Item.order)
        }
        @sort(id asc)
        Item { id: Id, order: Order, checker: User?, qty: int, @index order }
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

    let ids = SeqIdGen::default();
    let absent = dispatch(
        &c,
        &backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
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
        &ids,
        &NoStore,
        &Guards::new(),
        None,
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
        Item { id: Id, name: text, rank: int }
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

/// `with count`, proven against a real engine: the page envelope carries the live-row
/// `total` beside the bounded window; a page without `with count` omits the field.
#[tokio::test]
async fn with_count_page_carries_total_end_to_end() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Item { id: Id, name: text, rank: int }
        shape ItemCard from Item { name }
        query counted() -> ItemCard[] { list Item order (rank asc) page (2) offset with count; }
        query windowed() -> ItemCard[] { list Item order (rank asc) page (2); }
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

    // The counted page: two rows in the window, the whole live set in `total`.
    let p = call(&c, &backend, "POST", "/q/counted", json!({}), json!({})).await;
    assert_eq!(p.status, 200, "{:?}", p.body);
    assert_eq!(p.body["rows"], json!([{ "name": "a" }, { "name": "b" }]));
    assert_eq!(p.body["total"], json!(5));

    // Without `with count` the envelope has no `total` key at all.
    let w = call(&c, &backend, "POST", "/q/windowed", json!({}), json!({})).await;
    assert_eq!(w.status, 200, "{:?}", w.body);
    assert!(
        !w.body.as_object().unwrap().contains_key("total"),
        "{:?}",
        w.body
    );
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
        Order { id: Id, total: int, items: OrderItem[] }
        @sort(id asc)
        @soft_delete(deleted_at)
        OrderItem { id: Id, order: Order, sku: text, qty: int, deleted_at: timestamp? }
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
          id: Id
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
          id: Id
          subject: text
          comments: Comment[]
          pins: Pin[] @sort(rank desc)
        }
        @sort(pos asc)
        Comment { id: Id, ticket: Ticket, pos: int, body: text }
        @sort(rank asc)
        Pin { id: Id, ticket: Ticket, rank: int, label: text }
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
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int, items: OrderItem[] }
        @sort(id asc)
        @soft_delete(deleted_at)
        OrderItem { id: Id, order: Order, sku: text, qty: int, deleted_at: timestamp? }
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
          id: Id
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
        Order { id: Id, status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation close_order(id) -> OrderCard guard order_still_open {
            update Order where (id = $id) { status = "closed" };
        }
    "#;
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(!diags
        .iter()
        .any(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260"));
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
            // Fail closed: a guard that cannot decide denies.
            let Ok(mut conn) = backend.checkout("").await else {
                return GuardVerdict::deny("cannot verify order state");
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
            let ids = SeqIdGen::default();
            dispatch(
                c,
                &*backend,
                "",
                &ids,
                &NoStore,
                guards,
                None,
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

/// Whole-query raw body live: the raw SELECT is the statement, `${param}` binds
/// positionally, `{table}` interpolates the target's table, and the rows decode
/// into the declared shape by column name. The raw text owns soft-delete — the
/// hand-written tombstone filter excludes the tombstoned row.
#[tokio::test]
async fn raw_query_body_end_to_end() {
    let c = compile_sqlite(
        r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, total: int }
        shape UserRow from User { name, total }
        query heavy_users(min: int) -> UserRow[] {
          raw`SELECT u.name AS name, u.total AS total
              FROM {table} u
              WHERE u.total >= ${min} AND u.deleted_at IS NULL
              ORDER BY u.total DESC`;
        }
        query top_user() -> UserRow {
          raw`SELECT name, total FROM user WHERE deleted_at IS NULL ORDER BY total DESC LIMIT 1`;
        }
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
            "INSERT INTO `user` (`id`, `name`, `total`, `deleted_at`) VALUES
               ('a', 'Ada', 900, NULL),
               ('b', 'Bob', 500, NULL),
               ('c', 'Cud', 950, '2026-01-01T00:00:00Z'),
               ('d', 'Dee', 100, NULL);",
        )
        .await
        .expect("seed");

    // The bound `min` excludes Dee; the hand-written tombstone filter excludes Cud;
    // the raw ORDER BY holds.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/heavy_users",
        json!({ "min": 400 }),
        json!({}),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(
        got.body,
        json!([
            { "name": "Ada", "total": 900 },
            { "name": "Bob", "total": 500 }
        ])
    );

    // A scalar raw `get`: first row in the declared shape.
    let top = call(&c, &backend, "POST", "/q/top_user", json!({}), json!({})).await;
    assert_eq!(top.status, 200, "{:?}", top.body);
    assert_eq!(top.body, json!({ "name": "Ada", "total": 900 }));
}

// ---------- upsert (`create … on conflict update`) live proof --------------

async fn ddl_backend(c: &Compiled) -> SqliteBackend {
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("generated SQLite DDL failed: {e:?}\n{ddl}"));
    backend
}

#[tokio::test]
async fn upsert_inserts_then_composes_on_conflict() {
    // A page-view counter: the first hit inserts hits = 1, every later hit conflicts on the
    // unique `path` and increments the *stored* value server-side (the `hits = hits + 1`
    // atomic arithmetic in the conflict branch). The winning row reads back on both paths.
    let c = compile_sqlite(
        r#"
        Page { id: Id, path: text (unique), hits: int }
        shape PageRow from Page { path, hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
        }
        "#,
    );
    let backend = ddl_backend(&c).await;
    // One shared id generator across requests — the real deployment holds a single engine
    // id gen (a fresh one per call would re-mint the same id and collide on `page.id`).
    let ids = SeqIdGen::default();
    let hit = |path: &'static str| {
        let c = &c;
        let backend = &backend;
        let ids = &ids;
        async move {
            dispatch(
                c,
                backend,
                "",
                ids,
                &NoStore,
                &Guards::new(),
                None,
                "POST",
                "/m/record_hit",
                json!({ "path": path }),
                json!({}),
                None,
            )
            .await
        }
    };

    // Insert path.
    let first = hit("/home").await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({ "path": "/home", "hits": 1 }));

    // Conflict path — composes on the stored value, read-your-writes.
    assert_eq!(
        hit("/home").await.body,
        json!({ "path": "/home", "hits": 2 })
    );
    assert_eq!(
        hit("/home").await.body,
        json!({ "path": "/home", "hits": 3 })
    );

    // A different key is an independent insert.
    assert_eq!(
        hit("/about").await.body,
        json!({ "path": "/about", "hits": 1 })
    );
    // The first counter is untouched by the second key.
    assert_eq!(
        hit("/home").await.body,
        json!({ "path": "/home", "hits": 4 })
    );
}

/// An opaque `raw(…)` column and an opaque `@index raw(…)` execute against a real
/// database: the generated DDL creates both, a `create` writes the rest of the model
/// while the engine leaves the opaque column alone, and the declared shape reads it back
/// — bare (as an opaque string) and through the `raw` value leaf (a SQL function over it,
/// the only way to compute on a type the engine does not model).
#[tokio::test]
async fn opaque_column_and_index_execute_live() {
    let c = compile_sqlite(
        r#"
        Place {
          id:      Id
          name:    text
          shape_:  raw("blob")? (column "shape")
          @index raw("(lower(name))")
        }
        shape PlaceRow from Place {
          id
          name
          shape_
          shape_len = raw`length(shape)`
        }
        query place(id) -> PlaceRow;
        mutation add_place(name) -> PlaceRow {
          create Place { name = $name }
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    // The opaque column's declared type and the opaque index's body ride into the DDL
    // verbatim — SQLite executes both.
    assert!(ddl.contains("`shape` blob NULL"), "\n{ddl}");
    assert!(ddl.contains("ON `place` (lower(name));"), "\n{ddl}");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    // A create writes every modelled column and simply omits the opaque one.
    let created = call(
        &c,
        &backend,
        "POST",
        "/m/add_place",
        json!({ "name": "Dock" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(created.body["name"], json!("Dock"));
    assert_eq!(created.body["shape_"], json!(null));
    assert_eq!(created.body["shape_len"], json!(null));
    let id = created.body["id"]
        .as_str()
        .expect("generated id")
        .to_string();

    // A value the engine cannot construct still round-trips once the DB holds one: the
    // bare projection hands it back opaque, the raw leaf computes over it.
    backend
        .execute_batch(&format!(
            "UPDATE `place` SET `shape` = 'POINT(1 2)' WHERE `id` = '{id}';"
        ))
        .await
        .expect("set the opaque column out of band");
    let read = call(
        &c,
        &backend,
        "POST",
        "/q/place",
        json!({ "id": id }),
        json!({}),
    )
    .await;
    assert_eq!(read.status, 200, "{:?}", read.body);
    assert_eq!(read.body["name"], json!("Dock"));
    assert_eq!(read.body["shape_"], json!("POINT(1 2)"));
    assert_eq!(read.body["shape_len"], json!(10));
}

/// **Live FK enforcement + `on_delete: cascade`** against real SQLite. Proves two things
/// end to end: (1) the SQLite `foreign_keys` pragma is on (a bad-parent insert is rejected),
/// and (2) `@fk(on_delete: cascade)` in the generated DDL actually cascades — deleting the
/// parent row removes the child. Without the pragma SQLite silently ignores the FK, so this
/// test also guards the connection-setup pragma from regressing.
#[tokio::test]
async fn fk_on_delete_cascade_is_enforced_live() {
    use based_runtime::fetch_all;

    let src = "Org { id: Id  name: text }\n\
               Order { id: Id  org: Org @fk(on_delete: cascade) }";
    let sf = parse_file(src, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != based_diagnostics::Severity::Error),
        "sema errors: {diags:#?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    assert!(ddl.contains("ON DELETE CASCADE"), "\n{ddl}");

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            "INSERT INTO `org` (`id`, `name`) VALUES ('o1', 'Acme');\n\
             INSERT INTO `order` (`id`, `org_id`) VALUES ('r1', 'o1');",
        )
        .await
        .expect("seed");

    // (1) The FK is enforced: a child pointing at a non-existent parent is rejected.
    let bad = backend
        .execute_batch("INSERT INTO `order` (`id`, `org_id`) VALUES ('r2', 'ghost');")
        .await;
    assert!(bad.is_err(), "FK not enforced — the pragma is off");

    // (2) Deleting the parent cascades: the child row disappears.
    let mut db = backend.checkout("").await.expect("checkout");
    db.execute(
        "DELETE FROM `org` WHERE `id` = ?",
        &[based_runtime::SqlValue::Text("o1".into())],
    )
    .await
    .expect("delete parent");
    let rows = fetch_all(db.fetch("SELECT `id` FROM `order`", &[]))
        .await
        .expect("count orders");
    assert!(
        rows.is_empty(),
        "cascade did not remove the child rows: {rows:?}"
    );
}

/// **Far-side flattening projection** (`courses = enrollments.course { title }`) end to
/// end against a real engine: the many-to-many is flattened past its junction to a flat,
/// **distinct** `Vec<Course>`. Proves (1) the junction is hidden — the response carries
/// courses, never enrollment rows; (2) a course shared by two students appears for each;
/// (3) a duplicate junction link (two enrollments, same student→course) dedups to one far
/// row; (4) a soft-deleted junction row excludes its link; (5) a soft-deleted far course
/// is excluded entirely.
#[tokio::test]
async fn far_side_flattening_projection_returns_distinct_far_rows() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        @sort(id asc)
        @soft_delete(deleted_at)
        Enrollment { id: Id, student: Student, course: Course, deleted_at: timestamp? }
        @sort(title asc)
        @soft_delete(deleted_at)
        Course { id: Id, title: text, deleted_at: timestamp? }
        shape StudentCourses from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> StudentCourses;
        query students() -> StudentCourses[];
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
            INSERT INTO `student` (`id`, `name`) VALUES ('s1', 'Ann'), ('s2', 'Bob');
            INSERT INTO `course` (`id`, `title`, `deleted_at`) VALUES
                ('c1', 'Math', NULL), ('c2', 'Physics', NULL),
                -- a soft-deleted far course is excluded entirely.
                ('c3', 'Chemistry', '2020-01-01 00:00:00');
            INSERT INTO `enrollment` (`id`, `student_id`, `course_id`, `deleted_at`) VALUES
                ('e1', 's1', 'c1', NULL),
                -- a DUPLICATE link (s1 -> c1 again) must dedup to one Math.
                ('e2', 's1', 'c1', NULL),
                ('e3', 's1', 'c2', NULL),
                -- s1 -> c3 (Chemistry) — link is live, but the course is soft-deleted.
                ('e4', 's1', 'c3', NULL),
                -- a soft-deleted link s1 -> c2: c2 still reached via the live e3.
                ('e5', 's1', 'c2', '2020-01-01 00:00:00'),
                -- c1 (Math) shared with s2.
                ('e6', 's2', 'c1', NULL),
                -- s2 -> c2 link is soft-deleted → Physics absent for s2.
                ('e7', 's2', 'c2', '2020-01-01 00:00:00');
            "#,
        )
        .await
        .expect("seed");

    // Ann (s1): Math (deduped from e1+e2), Physics (live e3, though e5 is tombstoned);
    // Chemistry excluded (course soft-deleted). Junction hidden — flat course objects,
    // ordered by the far model's `@sort` (title asc).
    let ann = call(
        &c,
        &backend,
        "POST",
        "/q/student_by_id",
        json!({ "id": "s1" }),
        json!({}),
    )
    .await;
    assert_eq!(ann.status, 200, "{:?}", ann.body);
    assert_eq!(
        ann.body,
        json!({ "name": "Ann", "courses": [{ "title": "Math" }, { "title": "Physics" }] }),
        "distinct far courses, junction hidden, soft-deleted course + link excluded"
    );

    // Bob (s2): Math (shared c1 via e6); Physics absent (his only c2 link is tombstoned).
    let bob = call(
        &c,
        &backend,
        "POST",
        "/q/student_by_id",
        json!({ "id": "s2" }),
        json!({}),
    )
    .await;
    assert_eq!(bob.status, 200, "{:?}", bob.body);
    assert_eq!(
        bob.body,
        json!({ "name": "Bob", "courses": [{ "title": "Math" }] }),
        "shared course appears for the second student; soft-deleted link excludes Physics"
    );
}
