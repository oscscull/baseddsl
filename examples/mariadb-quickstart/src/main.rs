//! MariaDB quickstart — the whole engine, in one process, against a live MariaDB server.
//!
//! Copy this directory to start on MariaDB. It is the *same* `.bsl` schema + scenario as
//! `examples/sqlite-quickstart`, consumed through the **generated typed client**
//! (`src/client.rs`) over the in-process **`Engine`** — but pointed at a real MariaDB
//! server. The steps a user runs (see README):
//!
//!   1. set `DATABASE_URL` in `.env`
//!   2. `based migrate apply` — create the tables from the checked-in `migrations/`
//!   3. `based gen client -o src/client.rs --embedded` — the typed client (checked in)
//!   4. `cargo run` — this program: seed via the client's own `create` calls, then run
//!      the end-to-end scenario and exit 0 only if every step passes.
//!
//! There is **no raw SQL here** and **no hand-written transport bridge**. The only things
//! that differ from the SQLite slice are the *driver* (a pooled `ShardRouter`/`MariaDb` over
//! a live URL) and the *id generator* (`UuidGen` — MariaDB's native `UUID` id columns reject
//! non-uuid ids). The schema, the client, and the scenario are identical.

use based_runtime::driver::{PoolConfig, ShardRouter};
use based_runtime::id::UuidGen;
use based_runtime::{Compiled, Engine};
use std::path::PathBuf;

/// The typed client — the verbatim output of `based gen client -o src/client.rs --embedded`,
/// checked in as a reviewable artifact. It defines the wire surface *and* an in-process
/// `Embedded` transport over `Engine`, so `client::embedded(&engine)` is a ready client.
#[allow(dead_code)]
mod client;

// Typed ids: `Id<entity::User>` and `Id<entity::Org>` are distinct types, so an org id
// can't be passed where a user id is wanted (the client hands each one back typed).
use client::{entity, Id};

fn main() {
    // `.env` supplies `DATABASE_URL` (dotenvy, the 12-factor convention) — no hard-coded URL.
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL (see .env)");

    // Load the schema into the engine — the same front end `based check`/`based serve` run.
    // The tables already exist: `based migrate apply` created them (README step 2), so this
    // program never issues DDL.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compiled = Compiled::load(&manifest).expect("schema checks clean");

    // The `ShardRouter` is the production `Backend` (a bounded pool per physical shard); a
    // single-server deploy is `single`. A checked-out connection is the engine's `Db`.
    let router = ShardRouter::single(&url, PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to MariaDB at {url}: {e:?}"));

    // The `Engine` is the library twin of `based serve`, minus the socket: it owns the
    // schema, a connection, and an id generator, and runs each call through the same dispatch
    // core. `UuidGen` is the production generator (the MariaDB `UUID` id columns require it).
    let engine = Engine::new(
        compiled,
        router.checkout("").expect("checkout for engine"),
        UuidGen,
    );

    // `client::embedded(&engine)` (D62) is the entire bridge — a typed, in-process client
    // with no socket and no hand-written `Transport` impl.
    let api = client::embedded(&engine);

    // --- 0. seed via the client's own `create` mutations (no raw INSERT) ---
    // Two tenants + one user; the engine mints each uuid and hands it back.
    let acme = api
        .create_org(
            client::CreateOrgInput {
                name: "Acme".into(),
                slug: "acme".into(),
            },
            (),
        )
        .expect("create_org")
        .id;
    let other = api
        .create_org(
            client::CreateOrgInput {
                name: "Other".into(),
                slug: "other".into(),
            },
            (),
        )
        .expect("create_org (other)")
        .id;
    let ada = api
        .create_user(
            client::CreateUserInput {
                email: "ada@acme.test".into(),
                name: "Ada".into(),
            },
            (),
        )
        .expect("create_user")
        .id;

    // `$ctx` is the per-request context the *app* derives from its auth layer, never the
    // caller (auth.md/D7). Here every call acts as the org `acme`.
    let acme_ctx = || client::PlaceOrderCtx { org: acme.clone() };

    // --- 1. create → read the write back in its declared shape (read-your-writes) ---
    let placed = api
        .place_order(
            client::PlaceOrderInput {
                buyer: ada.clone(),
                total: 100,
            },
            acme_ctx(),
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
    let second = place(&api, &acme, &ada, 200);
    let third = place(&api, &acme, &ada, 300);

    // --- 2. read one back by id ---
    let got = get(&api, &acme, &placed.id).expect("the order exists");
    assert_eq!(got.id, placed.id);
    assert_eq!(got.total, 100);

    // --- 3. list/filter: the Tenant scope makes a plain `list` "my org's orders" ---
    let mine = api
        .my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: acme.clone() },
        )
        .expect("my_orders");
    assert_eq!(mine.len(), 3, "all three orders are visible to their org");
    // A different org sees none of them — the injected scope predicate is real.
    let others = api
        .my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: other.clone() },
        )
        .expect("my_orders (other org)");
    assert!(others.is_empty(), "cross-org rows are invisible");

    // --- 4. keyset pagination: walk all three orders two at a time ---
    let p1 = api
        .recent_orders(
            client::RecentOrdersInput { cursor: None },
            client::RecentOrdersCtx { org: acme.clone() },
        )
        .expect("recent_orders page 1");
    assert_eq!(p1.rows.len(), 2, "a full first page");
    let cursor = p1.cursor.clone().expect("more pages → a cursor");
    let p2 = api
        .recent_orders(
            client::RecentOrdersInput {
                cursor: Some(cursor),
            },
            client::RecentOrdersCtx { org: acme.clone() },
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
            client::CancelOrderCtx { org: acme.clone() },
        )
        .expect("cancel_order");
    assert_eq!(cancelled.id, placed.id);
    // It is now hidden from ordinary reads (the soft-delete live predicate).
    assert!(
        get(&api, &acme, &placed.id).is_none(),
        "a cancelled order is hidden from a get"
    );
    assert_eq!(
        api.my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: acme.clone() }
        )
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
            client::RestoreOrderCtx { org: acme.clone() },
        )
        .expect("restore_order");
    assert_eq!(restored.id, placed.id);
    assert!(
        get(&api, &acme, &placed.id).is_some(),
        "restored order is readable"
    );
    assert_eq!(
        api.my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: acme.clone() }
        )
        .expect("my_orders")
        .len(),
        3,
        "back to all three"
    );
    println!("soft-deleted then restored order {}", placed.id);

    println!("\nend-to-end scenario passed");
}

/// Place an order for `buyer` at `total`, acting as `org`; return its id.
fn place(
    api: &client::Client<client::Embedded>,
    org: &Id<entity::Org>,
    buyer: &Id<entity::User>,
    total: i64,
) -> Id<entity::Order> {
    api.place_order(
        client::PlaceOrderInput {
            buyer: buyer.clone(),
            total,
        },
        client::PlaceOrderCtx { org: org.clone() },
    )
    .expect("place_order")
    .id
}

/// Read an order back by id, acting as `org`.
fn get(
    api: &client::Client<client::Embedded>,
    org: &Id<entity::Org>,
    id: &Id<entity::Order>,
) -> Option<client::OrderCard> {
    api.order_by_id(
        client::OrderByIdInput { id: id.clone() },
        client::OrderByIdCtx { org: org.clone() },
    )
    .expect("order_by_id")
}
