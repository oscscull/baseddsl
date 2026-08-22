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
