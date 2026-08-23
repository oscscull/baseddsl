//! End-to-end proof of BW1 — bulk / structured shape-input `create` — against a **real**
//! SQLite engine, no mock.
//!
//! A `shape` doubles as the row-input type: `create Model[] from $rows` (bulk) and
//! `create Model from $row` (single) pull their column values from a shape-typed param,
//! materialized by the runtime as a chunked, atomic multi-row INSERT. The headline test is
//! the **round-trip north star**: rows read out in a shape feed back verbatim — the exact
//! same JSON, zero transformation — into a bulk create, and land identically.

#![cfg(feature = "sqlite")]

use serde_json::{json, Value};

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
Org { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)

Category { id: Id, name: text }

@created(created_at)
@updated(updated_at)
@scope Tenant
Product {
  id: Id
  org: Org
  category: Category
  sku: text
  name: text
  price: int
  created_at: timestamp
  updated_at: timestamp
  @index(org)
  @index(category)
}

# The input shape: a to-one relation is FK-linked with an inline key block.
shape ProductIn from Product {
  sku
  name
  price
  category { id }
}

shape OrgCard from Org { id, name }
shape CategoryCard from Category { id, name }

mutation create_org(name) -> OrgCard { create Org { name = $name }; }
mutation create_category(name) -> CategoryCard { create Category { name = $name }; }

mutation bulk_add_products(rows: ProductIn[]) -> ok scoped Tenant {
  create Product[] from $rows;
}
mutation add_one_product(row: ProductIn) -> ok scoped Tenant {
  create Product from $row;
}

query all_products() -> ProductIn[] scoped Tenant;

# BW2 — bulk upsert: a per-tenant sku is a composite unique key (scope + sku).
@scope Tenant
Inventory {
  id: Id
  org: Org
  sku: text
  qty: int
  price: int
  @index(org, sku) unique
  @index(org)
}
shape InvIn from Inventory { sku, qty, price }

# On a conflict, accumulate the stored qty with the incoming qty and take the incoming
# price. Reads the winning rows back in `InvIn` (BW1b), keyed on the conflict target.
mutation restock(rows: InvIn[]) -> InvIn[] scoped Tenant {
  create Inventory[] from $rows
    on conflict (org, sku) update { qty = qty + incoming.qty, price = incoming.price };
}
query all_inventory() -> InvIn[] scoped Tenant;

# BW1b — single-form read-back: `create Model from $row -> Shape` returns the one written row.
shape InvCard from Inventory { id, sku, qty }
mutation add_one_inv(row: InvIn) -> InvCard scoped Tenant { create Inventory from $row; }

# BW1b — bulk read-back of DB-generated `serial` ids (RETURNING on SQLite/Postgres).
@created(made_at)
Ticket {
  id: serial
  subject: text
  made_at: timestamp
}
shape TicketIn from Ticket { subject }
shape TicketOut from Ticket { id, subject }
mutation file_tickets(rows: TicketIn[]) -> TicketOut[] {
  create Ticket[] from $rows;
}

# Nested writes (to-one forward) — creating the related row from a payload block that
# names non-key columns. `Customer` is per-tenant (its scope is injected from `$ctx`, not
# the payload), so a nested write proves child scope injection too.
@scope Tenant
Customer {
  id: Id
  org: Org
  name: text
  email: text
  @index(org)
}
@scope Tenant
Order {
  id: Id
  org: Org
  customer: Customer
  total: int
  @index(org)
}
# The input shape reads/writes the customer as a nested object — the round-trip north star.
shape OrderIn from Order { total, customer { name, email } }
shape OrderCard from Order { total, customer { name, email } }
shape CustomerCard from Customer { name, email }

mutation place_order(row: OrderIn) -> ok scoped Tenant { create Order from $row; }
mutation place_orders(rows: OrderIn[]) -> ok scoped Tenant { create Order[] from $rows; }
query all_orders() -> OrderCard[] scoped Tenant;
query all_customers() -> CustomerCard[] scoped Tenant;

# A nested-write child with a DB-generated `serial` id (the parent FK is learned from the
# INSERT via RETURNING/LAST_INSERT_ID, not the payload).
Author { id: serial, name: text }
Book { id: Id, author: Author, title: text }
shape BookIn from Book { title, author { name } }
mutation add_books(rows: BookIn[]) -> ok { create Book[] from $rows; }
query all_books() -> BookIn[];
"#;

async fn load() -> (Compiled, SqliteBackend) {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean: {:?}",
        diags
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    (compiled, backend)
}

// One id generator per test, shared across every call — a fresh `SeqIdGen` per call would
// restart its counter and mint colliding ids across calls (org/category PK clashes).
async fn call(
    c: &Compiled,
    b: &SqliteBackend,
    ids: &SeqIdGen,
    path: &str,
    args: Value,
    ctx: Value,
) -> based_runtime::WireResponse {
    dispatch(
        c,
        b,
        "",
        ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        path,
        args,
        ctx,
        None,
    )
    .await
}

/// A stable, order-independent view of a set of product rows for comparison.
fn sorted_rows(mut v: Vec<Value>) -> Vec<Value> {
    v.sort_by_key(|r| r["sku"].as_str().unwrap_or_default().to_string());
    v
}

#[tokio::test]
async fn bulk_insert_round_trips_the_same_shape_verbatim() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();

    // Seed an org (the scope) and two categories.
    let org = call(
        &c,
        &b,
        &ids,
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    assert_eq!(org.status, 200, "{:?}", org.body);
    let org_id = org.body["id"].clone();
    let ctx = json!({ "org": org_id });

    let cat_a = call(
        &c,
        &b,
        &ids,
        "/m/create_category",
        json!({ "name": "A" }),
        json!({}),
    )
    .await;
    let cat_b = call(
        &c,
        &b,
        &ids,
        "/m/create_category",
        json!({ "name": "B" }),
        json!({}),
    )
    .await;
    let (ca, cb) = (cat_a.body["id"].clone(), cat_b.body["id"].clone());

    // A batch of rows in exactly the ProductIn shape (a to-one relation as `{ id }`).
    let rows = json!([
        { "sku": "S1", "name": "One",   "price": 100, "category": { "id": ca } },
        { "sku": "S2", "name": "Two",   "price": 200, "category": { "id": cb } },
        { "sku": "S3", "name": "Three", "price": 300, "category": { "id": ca } },
    ]);

    let resp = call(
        &c,
        &b,
        &ids,
        "/m/bulk_add_products",
        json!({ "rows": rows }),
        ctx.clone(),
    )
    .await;
    assert_eq!(resp.status, 200, "bulk insert failed: {:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({}),
        "an `-> ok` bulk create returns the empty ack"
    );

    // Read the rows back out in the SAME shape.
    let listed = call(&c, &b, &ids, "/q/all_products", json!({}), ctx.clone()).await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    let out = listed.body.as_array().expect("array").clone();
    assert_eq!(out.len(), 3);

    // North star: feed the EXACT returned Vec<ProductIn> back into a bulk create — zero
    // transformation — against a fresh scope, and it lands identically.
    let org2 = call(
        &c,
        &b,
        &ids,
        "/m/create_org",
        json!({ "name": "Beta" }),
        json!({}),
    )
    .await;
    let ctx2 = json!({ "org": org2.body["id"].clone() });
    let re = call(
        &c,
        &b,
        &ids,
        "/m/bulk_add_products",
        json!({ "rows": Value::Array(out.clone()) }),
        ctx2.clone(),
    )
    .await;
    assert_eq!(
        re.status, 200,
        "re-insert of the read shape failed: {:?}",
        re.body
    );

    let out2 = call(&c, &b, &ids, "/q/all_products", json!({}), ctx2)
        .await
        .body
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(
        sorted_rows(out),
        sorted_rows(out2),
        "the read shape, written back verbatim, produced identical rows"
    );
}

#[tokio::test]
async fn single_structured_create_inserts_one_row() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let org = call(
        &c,
        &b,
        &ids,
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    let ctx = json!({ "org": org.body["id"].clone() });
    let cat = call(
        &c,
        &b,
        &ids,
        "/m/create_category",
        json!({ "name": "A" }),
        json!({}),
    )
    .await;
    let ca = cat.body["id"].clone();

    let row = json!({ "sku": "ONE", "name": "Solo", "price": 42, "category": { "id": ca } });
    let resp = call(
        &c,
        &b,
        &ids,
        "/m/add_one_product",
        json!({ "row": row }),
        ctx.clone(),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);

    let out = call(&c, &b, &ids, "/q/all_products", json!({}), ctx)
        .await
        .body
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["sku"], json!("ONE"));
    assert_eq!(out[0]["price"], json!(42));
}

#[tokio::test]
async fn bulk_insert_chunks_transparently_and_stays_atomic() {
    // Enough rows to force the runtime's chunk boundary (SQLite bind cap is small): each
    // row binds 5 values (sku/name/price + minted id + `$ctx` org), so ~250 rows exceed
    // one chunk — the whole insert is still one atomic unit and every row lands.
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let org = call(
        &c,
        &b,
        &ids,
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    let ctx = json!({ "org": org.body["id"].clone() });
    let cat = call(
        &c,
        &b,
        &ids,
        "/m/create_category",
        json!({ "name": "A" }),
        json!({}),
    )
    .await;
    let ca = cat.body["id"].clone();

    let rows: Vec<Value> = (0..250)
        .map(|i| json!({ "sku": format!("S{i}"), "name": format!("N{i}"), "price": i, "category": { "id": ca } }))
        .collect();
    let resp = call(
        &c,
        &b,
        &ids,
        "/m/bulk_add_products",
        json!({ "rows": Value::Array(rows) }),
        ctx.clone(),
    )
    .await;
    assert_eq!(
        resp.status, 200,
        "chunked bulk insert failed: {:?}",
        resp.body
    );

    let out = call(&c, &b, &ids, "/q/all_products", json!({}), ctx)
        .await
        .body
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(out.len(), 250, "every chunked row landed");
}

#[tokio::test]
async fn empty_bulk_insert_is_a_success_not_a_404() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let org = call(
        &c,
        &b,
        &ids,
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    let ctx = json!({ "org": org.body["id"].clone() });
    let resp = call(
        &c,
        &b,
        &ids,
        "/m/bulk_add_products",
        json!({ "rows": [] }),
        ctx.clone(),
    )
    .await;
    assert_eq!(
        resp.status, 200,
        "an empty bulk insert is a no-op success: {:?}",
        resp.body
    );
    assert_eq!(resp.body, json!({}));

    let out = call(&c, &b, &ids, "/q/all_products", json!({}), ctx)
        .await
        .body
        .as_array()
        .expect("array")
        .clone();
    assert!(out.is_empty());
}

#[tokio::test]
async fn bulk_insert_injects_scope_from_ctx_not_the_payload() {
    // `@scope` is always engine-injected: even if a row tries to plant a different org, the
    // caller's `$ctx` scope wins, and another tenant never sees the rows.
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let acme = call(
        &c,
        &b,
        &ids,
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    let beta = call(
        &c,
        &b,
        &ids,
        "/m/create_org",
        json!({ "name": "Beta" }),
        json!({}),
    )
    .await;
    let acme_ctx = json!({ "org": acme.body["id"].clone() });
    let beta_ctx = json!({ "org": beta.body["id"].clone() });
    let cat = call(
        &c,
        &b,
        &ids,
        "/m/create_category",
        json!({ "name": "A" }),
        json!({}),
    )
    .await;
    let ca = cat.body["id"].clone();

    let rows = json!([{ "sku": "X1", "name": "n", "price": 1, "category": { "id": ca } }]);
    call(
        &c,
        &b,
        &ids,
        "/m/bulk_add_products",
        json!({ "rows": rows }),
        acme_ctx.clone(),
    )
    .await;

    // Acme sees its row; Beta's scope sees nothing.
    let acme_rows = call(&c, &b, &ids, "/q/all_products", json!({}), acme_ctx)
        .await
        .body
        .as_array()
        .expect("array")
        .len();
    let beta_rows = call(&c, &b, &ids, "/q/all_products", json!({}), beta_ctx)
        .await
        .body
        .as_array()
        .expect("array")
        .len();
    assert_eq!(acme_rows, 1);
    assert_eq!(beta_rows, 0, "the rows are confined to the caller's scope");
}

/// Seed an org (the scope) and return its `$ctx`.
async fn seed_org(c: &Compiled, b: &SqliteBackend, ids: &SeqIdGen, name: &str) -> Value {
    let org = call(
        c,
        b,
        ids,
        "/m/create_org",
        json!({ "name": name }),
        json!({}),
    )
    .await;
    assert_eq!(org.status, 200, "{:?}", org.body);
    json!({ "org": org.body["id"].clone() })
}

fn sorted_inv(mut v: Vec<Value>) -> Vec<Value> {
    v.sort_by_key(|r| r["sku"].as_str().unwrap_or_default().to_string());
    v
}

#[tokio::test]
async fn bulk_upsert_round_trips_and_updates_without_duplicating() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let ctx = seed_org(&c, &b, &ids, "Acme").await;

    // Seed three inventory rows via the fresh-insert path of the same upsert mutation.
    let seed = json!([
        { "sku": "A", "qty": 10, "price": 1 },
        { "sku": "B", "qty": 20, "price": 2 },
        { "sku": "C", "qty": 30, "price": 3 },
    ]);
    let r = call(
        &c,
        &b,
        &ids,
        "/m/restock",
        json!({ "rows": seed }),
        ctx.clone(),
    )
    .await;
    assert_eq!(r.status, 200, "seed upsert failed: {:?}", r.body);
    // The read-back returns the written rows in the declared shape (BW1b).
    assert_eq!(r.body.as_array().expect("array").len(), 3);

    // Read the exact Vec<InvIn> out.
    let out = call(&c, &b, &ids, "/q/all_inventory", json!({}), ctx.clone())
        .await
        .body
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(out.len(), 3);

    // North star: feed the SAME Vec back into the upsert — every row conflicts on (org, sku),
    // so each updates the existing row (qty += incoming.qty), never inserts a duplicate.
    let re = call(
        &c,
        &b,
        &ids,
        "/m/restock",
        json!({ "rows": Value::Array(out.clone()) }),
        ctx.clone(),
    )
    .await;
    assert_eq!(re.status, 200, "re-upsert failed: {:?}", re.body);

    let after = sorted_inv(
        call(&c, &b, &ids, "/q/all_inventory", json!({}), ctx)
            .await
            .body
            .as_array()
            .expect("array")
            .clone(),
    );
    assert_eq!(
        after.len(),
        3,
        "conflict path updated in place — no duplicates"
    );
    assert_eq!(after[0]["sku"], json!("A"));
    assert_eq!(after[0]["qty"], json!(20), "10 stored + 10 incoming");
    assert_eq!(after[1]["qty"], json!(40), "20 stored + 20 incoming");
    assert_eq!(after[2]["qty"], json!(60), "30 stored + 30 incoming");
}

#[tokio::test]
async fn bulk_upsert_accumulates_on_conflict_and_inserts_on_fresh() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let ctx = seed_org(&c, &b, &ids, "Acme").await;

    // Seed sku A with qty 5.
    call(
        &c,
        &b,
        &ids,
        "/m/restock",
        json!({ "rows": [{ "sku": "A", "qty": 5, "price": 1 }] }),
        ctx.clone(),
    )
    .await;

    // A mixed batch: A conflicts (5 + 3 = 8), B is fresh (inserted at 7).
    let mixed = json!([
        { "sku": "A", "qty": 3, "price": 9 },
        { "sku": "B", "qty": 7, "price": 4 },
    ]);
    let r = call(
        &c,
        &b,
        &ids,
        "/m/restock",
        json!({ "rows": mixed }),
        ctx.clone(),
    )
    .await;
    assert_eq!(r.status, 200, "mixed upsert failed: {:?}", r.body);
    // The read-back echoes both winning rows (the accumulated conflict + the fresh insert).
    let echoed = sorted_inv(r.body.as_array().expect("array").clone());
    assert_eq!(echoed.len(), 2);
    assert_eq!(echoed[0]["sku"], json!("A"));
    assert_eq!(echoed[0]["qty"], json!(8), "5 stored + 3 incoming");
    assert_eq!(echoed[0]["price"], json!(9), "took the incoming price");
    assert_eq!(echoed[1]["sku"], json!("B"));
    assert_eq!(echoed[1]["qty"], json!(7), "fresh insert");

    let after = sorted_inv(
        call(&c, &b, &ids, "/q/all_inventory", json!({}), ctx)
            .await
            .body
            .as_array()
            .expect("array")
            .clone(),
    );
    assert_eq!(after.len(), 2, "one conflict update + one fresh insert");
    assert_eq!(after[0]["qty"], json!(8));
    assert_eq!(after[1]["qty"], json!(7));
}

#[tokio::test]
async fn single_from_create_reads_back_one_row_in_shape() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let ctx = seed_org(&c, &b, &ids, "Acme").await;

    let row = json!({ "sku": "SOLO", "qty": 42, "price": 7 });
    let r = call(
        &c,
        &b,
        &ids,
        "/m/add_one_inv",
        json!({ "row": row }),
        ctx.clone(),
    )
    .await;
    assert_eq!(r.status, 200, "single read-back failed: {:?}", r.body);
    // A single `create Model from $row -> Shape` returns one object (not an array), with the
    // engine-minted id filled in.
    assert!(
        r.body.is_object(),
        "single read-back is one object: {:?}",
        r.body
    );
    assert_eq!(r.body["sku"], json!("SOLO"));
    assert_eq!(r.body["qty"], json!(42));
    assert!(
        r.body["id"].is_string(),
        "the minted id is read back: {:?}",
        r.body
    );
}

#[tokio::test]
async fn bulk_read_back_returns_db_generated_serial_ids() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();

    let rows = json!([{ "subject": "first" }, { "subject": "second" }, { "subject": "third" }]);
    let r = call(
        &c,
        &b,
        &ids,
        "/m/file_tickets",
        json!({ "rows": rows }),
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200, "bulk serial read-back failed: {:?}", r.body);
    let out = r.body.as_array().expect("array").clone();
    assert_eq!(out.len(), 3);

    // Row order matches input order; the DB assigned each a distinct integer id.
    assert_eq!(out[0]["subject"], json!("first"));
    assert_eq!(out[1]["subject"], json!("second"));
    assert_eq!(out[2]["subject"], json!("third"));
    let id0 = out[0]["id"].as_i64().expect("serial id is an integer");
    let id1 = out[1]["id"].as_i64().expect("serial id is an integer");
    let id2 = out[2]["id"].as_i64().expect("serial id is an integer");
    assert!(
        id0 < id1 && id1 < id2,
        "distinct, ordered DB-generated ids: {id0},{id1},{id2}"
    );
}

fn sorted_by<'a>(mut v: Vec<Value>, key: &'a str) -> Vec<Value> {
    v.sort_by_key(|r| r[key].as_str().unwrap_or_default().to_string());
    v
}

#[tokio::test]
async fn single_nested_write_creates_the_related_row_and_links_it() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let ctx = seed_org(&c, &b, &ids, "Acme").await;

    // One order whose customer is created inline (a to-one nested write).
    let row = json!({ "total": 100, "customer": { "name": "Ada", "email": "ada@x.io" } });
    let r = call(
        &c,
        &b,
        &ids,
        "/m/place_order",
        json!({ "row": row }),
        ctx.clone(),
    )
    .await;
    assert_eq!(r.status, 200, "nested create failed: {:?}", r.body);

    // The customer row was created (child insert) …
    let cust = call(&c, &b, &ids, "/q/all_customers", json!({}), ctx.clone()).await;
    assert_eq!(cust.body, json!([{ "name": "Ada", "email": "ada@x.io" }]));

    // … and the order links to it — reading the order back nests the created customer.
    let orders = call(&c, &b, &ids, "/q/all_orders", json!({}), ctx.clone()).await;
    assert_eq!(
        orders.body,
        json!([{ "total": 100, "customer": { "name": "Ada", "email": "ada@x.io" } }])
    );
}

#[tokio::test]
async fn bulk_nested_write_links_each_parent_to_its_own_child() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();
    let ctx = seed_org(&c, &b, &ids, "Acme").await;

    let rows = json!([
        { "total": 10, "customer": { "name": "Ann", "email": "ann@x.io" } },
        { "total": 20, "customer": { "name": "Bob", "email": "bob@x.io" } },
        { "total": 30, "customer": { "name": "Cy",  "email": "cy@x.io" } },
    ]);
    let r = call(
        &c,
        &b,
        &ids,
        "/m/place_orders",
        json!({ "rows": rows }),
        ctx.clone(),
    )
    .await;
    assert_eq!(r.status, 200, "bulk nested create failed: {:?}", r.body);

    // Three distinct customers created …
    let cust = call(&c, &b, &ids, "/q/all_customers", json!({}), ctx.clone()).await;
    assert_eq!(
        sorted_by(cust.body.as_array().unwrap().clone(), "name"),
        json!([
            { "name": "Ann", "email": "ann@x.io" },
            { "name": "Bob", "email": "bob@x.io" },
            { "name": "Cy",  "email": "cy@x.io" },
        ])
        .as_array()
        .unwrap()
        .clone()
    );

    // … each order linked to the RIGHT customer (per-row FK alignment).
    let mut got: Vec<Value> = call(&c, &b, &ids, "/q/all_orders", json!({}), ctx.clone())
        .await
        .body
        .as_array()
        .unwrap()
        .clone();
    got.sort_by_key(|r| r["total"].as_i64().unwrap_or(0));
    assert_eq!(
        got,
        json!([
            { "total": 10, "customer": { "name": "Ann", "email": "ann@x.io" } },
            { "total": 20, "customer": { "name": "Bob", "email": "bob@x.io" } },
            { "total": 30, "customer": { "name": "Cy",  "email": "cy@x.io" } },
        ])
        .as_array()
        .unwrap()
        .clone()
    );
}

#[tokio::test]
async fn nested_write_child_with_db_generated_serial_id() {
    let (c, b) = load().await;
    let ids = SeqIdGen::default();

    // Each book's author is created with a DB-generated serial id; the book's FK is learned
    // from the INSERT (RETURNING on SQLite) and linked per row.
    let rows = json!([
        { "title": "SICP",  "author": { "name": "Sussman" } },
        { "title": "TAOCP", "author": { "name": "Knuth" } },
    ]);
    let r = call(
        &c,
        &b,
        &ids,
        "/m/add_books",
        json!({ "rows": rows }),
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200, "serial nested create failed: {:?}", r.body);

    let books = call(&c, &b, &ids, "/q/all_books", json!({}), json!({})).await;
    assert_eq!(
        sorted_by(books.body.as_array().unwrap().clone(), "title"),
        json!([
            { "title": "SICP",  "author": { "name": "Sussman" } },
            { "title": "TAOCP", "author": { "name": "Knuth" } },
        ])
        .as_array()
        .unwrap()
        .clone()
    );
}
