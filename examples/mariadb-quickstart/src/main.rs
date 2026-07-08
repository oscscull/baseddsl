//! MariaDB quickstart — the whole engine, in one process, against a live MariaDB server.
//!
//! This is the thing to copy to start on MariaDB: the *same* `.bsl` schema + scenario as
//! `examples/sqlite-quickstart`, consumed through the **generated typed client** running
//! over the in-process **`Engine`** — but pointed at a real **MariaDB** server instead of
//! bundled SQLite. `build.rs` regenerates the client + MariaDB DDL from the schema on every
//! build, so nothing here is stale. Point `DATABASE_URL` at a MariaDB and run `cargo run`:
//! it creates the tables, runs an end-to-end scenario (create → read-your-writes → list/
//! filter → paginate → soft-delete/restore), and exits 0 only if every step passes.
//!
//! The only things that differ from the SQLite slice are the *driver* (a pooled
//! `ShardRouter`/`MariaDb` over a live URL, not an in-memory `SqliteDb`), the *id generator*
//! (`UuidGen` — MariaDB's native `UUID` id columns reject non-uuid ids), and the ids in the
//! fixtures (real v4 UUIDs, for the same reason). The schema, the client, the `InProcess`
//! bridge, and the scenario assertions are identical — that is the point of the reference.

use based_runtime::driver::{PoolConfig, ShardRouter};
use based_runtime::id::UuidGen;
use based_runtime::{Compiled, Db, Engine};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// The typed client, generated at build time from `schema/*.bsl` (see `build.rs`) — the
/// verbatim output of `based gen client`. Regenerated on every build, never checked in.
#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}

/// The MariaDB DDL, generated at build time (`based gen sql`). Creates every table.
const SCHEMA_SQL: &str = include_str!(concat!(env!("OUT_DIR"), "/schema.sql"));

/// The connection URL default: the throwaway server this example's README stands up. Override
/// it with `DATABASE_URL` to point at your own MariaDB.
const DEFAULT_URL: &str = "mysql://root:based_test_pw@127.0.0.1:3307/based_test";

// The org + user the scenario acts as. MariaDB maps `Id` to a native `UUID` column, which
// rejects a non-uuid string — so, unlike the SQLite slice's `org-acme`/`user-ada`, the seed
// ids are real v4-shaped UUIDs (the trailing digits keep them readable across assertions).
const ORG_ACME: &str = "00000000-0000-4000-8000-0000000000a1";
const ORG_OTHER: &str = "00000000-0000-4000-8000-0000000000a2";
const USER_ADA: &str = "00000000-0000-4000-8000-0000000000b1";

/// Reset the three tables before recreating them, so the example is re-runnable against a
/// persistent server (the generated DDL is a plain `CREATE TABLE`, no `IF NOT EXISTS`). No
/// FK constraints are emitted (relations are bare `_id` columns), so drop order is free.
const RESET_SQL: &str = "DROP TABLE IF EXISTS `order`; \
                         DROP TABLE IF EXISTS `user`; \
                         DROP TABLE IF EXISTS `org`;";

/// Seed the org + user the scenario references (real UUID ids, per the note above).
const SEED_SQL: &str = "\
    INSERT INTO `org`  (`id`, `name`, `slug`)  VALUES ('00000000-0000-4000-8000-0000000000a1', 'Acme', 'acme'); \
    INSERT INTO `user` (`id`, `email`, `name`) VALUES ('00000000-0000-4000-8000-0000000000b1', 'ada@acme.test', 'Ada');";

/// The bridge from the generated `Transport` to an in-process [`Engine`] — the whole of
/// what an embedding app writes (the `Transport` trait is defined *by* the generated
/// client, so the orphan rule keeps this impl here, next to the generated module). It
/// serializes the typed input + `$ctx` to JSON, runs it through the engine, and decodes
/// the `200` body into the output type (a non-`200` becomes a `ClientError`). Identical to
/// the SQLite slice's bridge — the `Engine` is driver-agnostic.
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
    // --- Stand up an in-process engine over a live MariaDB database ---
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema");
    let compiled = Compiled::load(&schema_dir).expect("schema checks clean");

    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    // The `ShardRouter` is the production `Backend` (a bounded pool per physical shard); a
    // single-server deploy is `single`. A checked-out connection is the engine's `Db`.
    let router = ShardRouter::single(&url, PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to MariaDB at {url}: {e:?}"));

    // Create every table from the *generated* MariaDB DDL and seed the fixtures on one
    // checkout, then hand a fresh checkout to the engine. A real deployment applies the DDL
    // out of band (`based migrate apply`, `mysql < schema.sql`); a self-contained smoke test
    // does it inline.
    {
        let mut setup = router.checkout("").expect("checkout for setup");
        run_script(&mut setup, RESET_SQL);
        run_script(&mut setup, SCHEMA_SQL);
        run_script(&mut setup, SEED_SQL);
    }

    // The `Engine` is the library twin of `based serve`, minus the socket: it owns the
    // schema, a connection, and an id generator, and runs each call through the same dispatch
    // core. `UuidGen` is the production generator (the MariaDB `UUID` id columns require it).
    let engine = Engine::new(
        compiled,
        router.checkout("").expect("checkout for engine"),
        UuidGen,
    );
    let api = client::Client {
        transport: InProcess { engine: &engine },
    };

    // `$ctx` is the per-request context the *app* derives from its auth layer, never the
    // caller (auth.md/D7). Here every call acts as the org `acme`.
    let acme = || client_org(ORG_ACME);

    // --- 1. create → read the write back in its declared shape (read-your-writes) ---
    let placed = api
        .place_order(
            client::PlaceOrderInput {
                buyer: USER_ADA.into(),
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
                org: client_org(ORG_OTHER),
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

/// Run a `;`-separated setup script against the live server one statement at a time (the
/// pooled driver's `execute` runs a single statement; `--` comment lines and blank
/// fragments are skipped). The generated DDL uses `;` only as a terminator, so a plain split
/// is safe for this fixture SQL.
fn run_script(db: &mut impl Db, script: &str) {
    for frag in script.split(';') {
        let stmt: String = frag
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        db.execute(stmt, &[])
            .unwrap_or_else(|e| panic!("setup statement failed: {e:?}\n{stmt}"));
    }
}

/// A `$ctx.org` value (the scope owner the app derives from auth, D33/D46).
fn client_org(id: &str) -> client::Uuid {
    id.to_string()
}

/// Place an order for the seeded buyer at `total`, acting as org `acme`; return its id.
fn place(api: &client::Client<InProcess>, total: i64) -> String {
    api.place_order(
        client::PlaceOrderInput {
            buyer: USER_ADA.into(),
            total,
        },
        client::PlaceOrderCtx {
            org: client_org(ORG_ACME),
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
            org: client_org(ORG_ACME),
        },
    )
    .expect("order_by_id")
}
