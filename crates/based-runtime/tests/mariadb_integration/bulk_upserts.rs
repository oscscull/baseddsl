use super::*;

// --- BW2 bulk upsert + BW1b bulk read-back (D127) against live MariaDB -------------------
//
// MariaDB genuinely diverges here: no `INSERT … RETURNING`, so a `serial` bulk read-back
// re-selects via the `LAST_INSERT_ID()` range, and `on conflict` lowers to `ON DUPLICATE KEY
// UPDATE` with `incoming.<col>` → `VALUES(<col>)`. This suite is the per-dialect live proof.
const BULK_SCHEMA: &str = r#"
Org { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)
shape OrgCard from Org { id, name }
mutation create_org(name) -> OrgCard { create Org { name = $name }; }

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
mutation restock(rows: InvIn[]) -> InvIn[] scoped Tenant {
  create Inventory[] from $rows
    on conflict (org, sku) update { qty = qty + incoming.qty, price = incoming.price };
}
query all_inventory() -> InvIn[] scoped Tenant;

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
"#;

#[tokio::test]
async fn bulk_upsert_and_serial_read_back_live_mariadb() {
    let Some((c, router, _guard)) = live_schema(BULK_SCHEMA).await else {
        return;
    };

    // Seed the scope (a real minted uuid org).
    let org = call(
        &c,
        &router,
        "POST",
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    assert_eq!(org.status, 200, "{:?}", org.body);
    let ctx = json!({ "org": org.body["id"].clone() });

    // Fresh insert path (ON DUPLICATE KEY UPDATE fires nothing), read back in shape.
    let seed = json!([
        { "sku": "A", "qty": 10, "price": 1 },
        { "sku": "B", "qty": 20, "price": 2 },
    ]);
    let r = call(
        &c,
        &router,
        "POST",
        "/m/restock",
        json!({ "rows": seed }),
        ctx.clone(),
    )
    .await;
    assert_eq!(r.status, 200, "seed upsert failed: {:?}", r.body);
    assert_eq!(r.body.as_array().expect("array").len(), 2);

    // Re-upsert a conflicting + a fresh row: A accumulates (10+5), C is inserted. This is
    // `ON DUPLICATE KEY UPDATE qty = qty + VALUES(qty), price = VALUES(price)`.
    let mixed = json!([
        { "sku": "A", "qty": 5, "price": 9 },
        { "sku": "C", "qty": 7, "price": 3 },
    ]);
    let re = call(
        &c,
        &router,
        "POST",
        "/m/restock",
        json!({ "rows": mixed }),
        ctx.clone(),
    )
    .await;
    assert_eq!(re.status, 200, "conflict upsert failed: {:?}", re.body);

    let mut after = call(
        &c,
        &router,
        "POST",
        "/q/all_inventory",
        json!({}),
        ctx.clone(),
    )
    .await
    .body
    .as_array()
    .expect("array")
    .clone();
    after.sort_by_key(|v| v["sku"].as_str().unwrap_or_default().to_string());
    assert_eq!(
        after.len(),
        3,
        "one conflict update + two fresh inserts, no dups"
    );
    assert_eq!(after[0]["sku"], json!("A"));
    assert_eq!(
        after[0]["qty"],
        json!(15),
        "10 stored + 5 incoming (VALUES(qty))"
    );
    assert_eq!(after[0]["price"], json!(9), "took VALUES(price)");
    assert_eq!(after[2]["sku"], json!("C"));
    assert_eq!(after[2]["qty"], json!(7));

    // Serial bulk read-back via the LAST_INSERT_ID() range (no RETURNING on MariaDB).
    let tickets = json!([{ "subject": "first" }, { "subject": "second" }, { "subject": "third" }]);
    let t = call(
        &c,
        &router,
        "POST",
        "/m/file_tickets",
        json!({ "rows": tickets }),
        json!({}),
    )
    .await;
    assert_eq!(t.status, 200, "serial read-back failed: {:?}", t.body);
    let out = t.body.as_array().expect("array").clone();
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["subject"], json!("first"));
    assert_eq!(out[2]["subject"], json!("third"));
    let id0 = out[0]["id"].as_i64().expect("serial id");
    let id2 = out[2]["id"].as_i64().expect("serial id");
    assert_eq!(
        id2,
        id0 + 2,
        "contiguous DB-generated id range in input order"
    );
}
