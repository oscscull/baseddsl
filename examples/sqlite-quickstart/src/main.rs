//! SQLite quickstart — the whole engine, in one process, no socket.
//!
//! This is the thing to copy to start: a `.bsl` schema (`schema/`) consumed through the
//! **generated typed client** running over the in-process **`Engine`** against a live
//! SQLite database. `build.rs` regenerates the client + DDL from the schema on every
//! build, so nothing here is stale. Run it with `cargo run` — it executes an end-to-end
//! scenario (create → read-your-writes → list/filter → paginate → soft-delete/restore)
//! and exits 0 only if every step passes.

use based_runtime::{Compiled, Engine, SeqIdGen, SqliteDb};
use rusqlite::Connection;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// The typed client, generated at build time from `schema/*.bsl` (see `build.rs`) — the
/// verbatim output of `based gen client`. Regenerated on every build, never checked in.
#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}

/// The SQLite DDL, generated at build time (`based gen sql`). Creates every table.
const SCHEMA_SQL: &str = include_str!(concat!(env!("OUT_DIR"), "/schema.sql"));

/// The org + user the scenario acts as. Real ids are engine-generated uuids; these are
/// fixed so the scenario can reference them.
const SEED_SQL: &str = r#"
    INSERT INTO `org`  (`id`, `name`, `slug`)  VALUES ('org-acme', 'Acme', 'acme');
    INSERT INTO `user` (`id`, `email`, `name`) VALUES ('user-ada', 'ada@acme.test', 'Ada');
"#;

/// The bridge from the generated `Transport` to an in-process [`Engine`] — the whole of
/// what an embedding app writes (the `Transport` trait is defined *by* the generated
/// client, so the orphan rule keeps this impl here, next to the generated module). It
/// serializes the typed input + `$ctx` to JSON, runs it through the engine, and decodes
/// the `200` body into the output type (a non-`200` becomes a `ClientError`).
struct InProcess<'a> {
    engine: &'a Engine,
}

impl client::Transport for InProcess<'_> {
    fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, client::ClientError>
    where
        I: Serialize,
        C: Serialize,
        O: DeserializeOwned,
    {
        let args = serde_json::to_value(input).map_err(|e| client::ClientError(e.to_string()))?;
        // `&()` → JSON `null`; the engine treats a non-object context as empty.
        let ctx = serde_json::to_value(ctx)
            .map(|v| if v.is_object() { v } else { json!({}) })
            .map_err(|e| client::ClientError(e.to_string()))?;
        let resp = self.engine.call(route, args, ctx);
        if resp.status == 200 {
            serde_json::from_value(resp.body).map_err(|e| client::ClientError(e.to_string()))
        } else {
            let msg = resp.body["error"]["message"]
                .as_str()
                .unwrap_or("call failed");
            Err(client::ClientError(msg.to_string()))
        }
    }
}

fn main() {
    // --- Stand up an in-process engine over a live in-memory SQLite database ---
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema");
    let compiled = Compiled::load(&schema_dir).expect("schema checks clean");

    let conn = Connection::open_in_memory().expect("open sqlite");
    conn.execute_batch(SCHEMA_SQL).expect("create tables");
    conn.execute_batch(SEED_SQL).expect("seed org + user");

    // The `Engine` is the library twin of `based serve`, minus the socket: it owns the
    // schema, the connection, and an id generator, and runs each call through the same
    // dispatch core. `SeqIdGen` yields deterministic ids for a demo; production uses the
    // uuid generator (behind the runtime's `serve` feature) or any custom `IdGen`.
    let engine = Engine::new(compiled, SqliteDb::new(conn), SeqIdGen::default());
    let api = client::Client {
        transport: InProcess { engine: &engine },
    };

    // `$ctx` is the per-request context the *app* derives from its auth layer, never the
    // caller (auth.md/D7). Here every call acts as the org `acme`.
    let acme = || client_org("org-acme");

    // --- 1. create → read the write back in its declared shape (read-your-writes) ---
    let placed = api
        .place_order(
            client::PlaceOrderInput {
                buyer: "user-ada".into(),
                total: 100,
            },
            client::PlaceOrderCtx { org: acme() },
        )
        .expect("place_order");
    assert_eq!(placed.status, "pending", "status defaults on create");
    assert_eq!(placed.total, 100);
    // The nested to-one sub-object (`placed_by { name, email }`) comes back as a real
    // object, joined + projected in the same transaction as the write.
    assert_eq!(placed.placed_by.name, "Ada");
    assert_eq!(placed.placed_by.email, "ada@acme.test");
    println!("created order {} for {}", placed.id, placed.placed_by.name);

    // Two more, so there is a set to paginate.
    let second = place(&api, 200);
    let third = place(&api, 300);

    // --- 2. read one back by id ---
    let got = api
        .order_by_id(
            client::OrderByIdInput {
                id: placed.id.clone(),
            },
            client::OrderByIdCtx { org: acme() },
        )
        .expect("order_by_id")
        .expect("the order exists");
    assert_eq!(got.id, placed.id);
    assert_eq!(got.total, 100);

    // --- 3. list/filter: the Tenant scope makes a plain `list` "my org's orders" ---
    let mine = api
        .my_orders(client::MyOrdersInput, client::MyOrdersCtx { org: acme() })
        .expect("my_orders");
    assert_eq!(mine.len(), 3, "all three orders are visible to their org");
    // A different org sees none of them — the injected scope predicate is real.
    let other = api
        .my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx {
                org: client_org("org-other"),
            },
        )
        .expect("my_orders (other org)");
    assert!(other.is_empty(), "cross-org rows are invisible");

    // --- 4. keyset pagination: walk all three orders two at a time ---
    let p1 = api
        .recent_orders(
            client::RecentOrdersInput { cursor: None },
            client::RecentOrdersCtx { org: acme() },
        )
        .expect("recent_orders page 1");
    assert_eq!(p1.rows.len(), 2, "a full first page");
    let cursor = p1.cursor.clone().expect("more pages → a cursor");
    let p2 = api
        .recent_orders(
            client::RecentOrdersInput {
                cursor: Some(cursor),
            },
            client::RecentOrdersCtx { org: acme() },
        )
        .expect("recent_orders page 2");
    assert_eq!(p2.rows.len(), 1, "a short final page");
    assert!(p2.cursor.is_none(), "the last page carries no cursor");
    // Every order appeared exactly once across the two pages.
    let mut paged: Vec<_> = p1
        .rows
        .iter()
        .chain(&p2.rows)
        .map(|o| o.id.clone())
        .collect();
    paged.sort();
    let mut all = [placed.id.clone(), second, third];
    all.sort();
    assert_eq!(paged, all, "pagination walked the whole set exactly once");
    println!("paged {} orders across 2 pages", paged.len());

    // --- 5. soft-delete + restore round-trip ---
    // `delete` on a @soft_delete model tombstones the row and reads it back in its
    // declared shape (D58) — the row still projects even though it is now soft-deleted.
    let cancelled = api
        .cancel_order(
            client::CancelOrderInput {
                id: placed.id.clone(),
            },
            client::CancelOrderCtx { org: acme() },
        )
        .expect("cancel_order");
    assert_eq!(cancelled.id, placed.id);
    // It is now hidden from ordinary reads (the soft-delete live predicate).
    assert!(
        get(&api, &placed.id).is_none(),
        "a cancelled order is hidden from a get"
    );
    assert_eq!(
        api.my_orders(client::MyOrdersInput, client::MyOrdersCtx { org: acme() })
            .expect("my_orders")
            .len(),
        2,
        "and from the list"
    );

    // `restore` lifts the tombstone; the row is readable again.
    let restored = api
        .restore_order(
            client::RestoreOrderInput {
                id: placed.id.clone(),
            },
            client::RestoreOrderCtx { org: acme() },
        )
        .expect("restore_order");
    assert_eq!(restored.id, placed.id);
    assert!(
        get(&api, &placed.id).is_some(),
        "restored order is readable"
    );
    assert_eq!(
        api.my_orders(client::MyOrdersInput, client::MyOrdersCtx { org: acme() })
            .expect("my_orders")
            .len(),
        3,
        "back to all three"
    );
    println!("soft-deleted then restored order {}", placed.id);

    println!("\nend-to-end scenario passed");
}

/// A `$ctx.org` value (the scope owner the app derives from auth, D33/D46).
fn client_org(id: &str) -> client::Uuid {
    id.to_string()
}

/// Place an order for the seeded buyer at `total`, acting as org `acme`; return its id.
fn place(api: &client::Client<InProcess>, total: i64) -> String {
    api.place_order(
        client::PlaceOrderInput {
            buyer: "user-ada".into(),
            total,
        },
        client::PlaceOrderCtx {
            org: client_org("org-acme"),
        },
    )
    .expect("place_order")
    .id
}

/// Read an order back by id, acting as org `acme`.
fn get(api: &client::Client<InProcess>, id: &str) -> Option<client::OrderCard> {
    api.order_by_id(
        client::OrderByIdInput { id: id.to_string() },
        client::OrderByIdCtx {
            org: client_org("org-acme"),
        },
    )
    .expect("order_by_id")
}
