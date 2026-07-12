//! SQLite quickstart — the whole engine, in one process, over a bundled SQLite database.
//!
//! Copy this directory to start. A `.bsl` schema (`schema/`) is consumed through the
//! **generated typed client** (`src/client.rs`) running over the in-process **`Engine`**.
//! The steps a user runs (see README):
//!
//!   1. set `DATABASE_URL` in `.env`
//!   2. `based migrate apply` — create the tables from the checked-in `migrations/`
//!   3. `based gen client -o src/client.rs --embedded` — the typed client (checked in)
//!   4. `cargo run` — this program: seed via the client's own `create` calls, then run
//!      the end-to-end scenario (create → read-your-writes → get → list/scope → paginate
//!      → soft-delete/restore) and exit 0 only if every step passes.
//!
//! ## Error handling — the reference this file doubles as
//!
//! Every client call returns `Result<_, ClientError>`, so the whole flow threads `?` and
//! `main` returns a `Result`. A `ClientError` is a `std::error::Error` you branch on by
//! class: `kind()` (`Transport` / `Decode` / `Api`), a stable machine `code()`, and, for a
//! server-side failure, the HTTP `status()`. Step 6 shows the pattern — a deliberately
//! malformed cursor is rejected and matched on `kind()`.

use based_runtime::{Compiled, Engine, SeqIdGen, SqliteBackend};
use std::path::PathBuf;

/// The typed client — the verbatim output of `based gen client -o src/client.rs --embedded`,
/// checked in as a reviewable artifact. It defines the wire surface *and* an in-process
/// `Embedded` transport over `Engine`, so `client::embedded(&engine)` is a ready client.
#[allow(dead_code)]
mod client;

// Typed ids: `Id<entity::User>` and `Id<entity::Org>` are distinct types, so the compiler
// rejects passing an org id where a user id is wanted (the client hands each one back typed).
use client::{entity, Id};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `.env` supplies `DATABASE_URL` (dotenvy, the 12-factor convention).
    dotenvy::dotenv().ok();
    let db_path = std::env::var("DATABASE_URL").map_err(|_| "set DATABASE_URL (see .env)")?;

    // Load the schema into the engine — the same front end `based check`/`based serve` run.
    // The tables already exist: `based migrate apply` created them (README step 2), so this
    // program only issues queries.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compiled = Compiled::load(&manifest).map_err(|e| format!("load schema: {e:?}"))?;

    // The `Engine` is the library twin of `based serve`, minus the socket: it owns the
    // schema, a connection `Backend`, and an id generator, and runs each call through the
    // same async dispatch core. `SqliteBackend::open` is the whole database setup — bundled
    // SQLite, file created if absent. `SeqIdGen` yields readable ids for a demo; production
    // uses `UuidGen` (behind the runtime's `serve` feature) or any custom `IdGen`.
    let engine = Engine::new(
        compiled,
        SqliteBackend::open(&db_path)?,
        SeqIdGen::default(),
    );

    // `client::embedded(&engine)` is the entire bridge — a typed, in-process client that
    // implements the `Transport` seam over `Engine` for you.
    let api = client::embedded(&engine);

    // --- 0. seed via the client's own `create` mutations ---
    // Two tenants + one user; the engine mints each id and hands it back.
    let acme = api
        .create_org(
            client::CreateOrgInput {
                name: "Acme".into(),
                slug: "acme".into(),
            },
            (),
        )
        .await?
        .id;
    let other = api
        .create_org(
            client::CreateOrgInput {
                name: "Other".into(),
                slug: "other".into(),
            },
            (),
        )
        .await?
        .id;
    let ada = api
        .create_user(
            client::CreateUserInput {
                email: "ada@acme.test".into(),
                name: "Ada".into(),
            },
            (),
        )
        .await?
        .id;

    // `$ctx` is the per-request context the *app* derives from its auth layer. Here every
    // call acts as the org `acme`.
    let acme_ctx = || client::PlaceOrderCtx { org: acme.clone() };

    // --- 1. create → read the write back in its declared shape (read-your-writes) ---
    let placed = api
        .place_order(
            client::PlaceOrderInput {
                buyer: ada.clone(),
                total: money(100),
            },
            acme_ctx(),
        )
        .await?;
    assert_eq!(placed.status, "pending", "status defaults on create");
    assert_eq!(placed.total, money(100));
    // The nested to-one sub-object (`placed_by { name, email }`) comes back as a real
    // object, joined + projected in the same transaction as the write.
    assert_eq!(placed.placed_by.name, "Ada");
    assert_eq!(placed.placed_by.email, "ada@acme.test");
    println!("created order {} for {}", placed.id, placed.placed_by.name);

    // Two more, so there is a set to paginate.
    let second = place(&api, &acme, &ada, 200).await?;
    let third = place(&api, &acme, &ada, 300).await?;

    // --- 2. read one back by id ---
    let got = get(&api, &acme, &placed.id)
        .await?
        .expect("the order exists");
    assert_eq!(got.id, placed.id);
    assert_eq!(got.total, money(100));

    // --- 3. list/filter: the Tenant scope makes a plain `list` "my org's orders" ---
    let mine = api
        .my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: acme.clone() },
        )
        .await?;
    assert_eq!(mine.len(), 3, "all three orders are visible to their org");
    // A different org sees none of them — the injected scope predicate is real.
    let others = api
        .my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: other.clone() },
        )
        .await?;
    assert!(others.is_empty(), "cross-org rows stay hidden");

    // --- 4. keyset pagination: walk all three orders two at a time ---
    let p1 = api
        .recent_orders(
            client::RecentOrdersInput { cursor: None },
            client::RecentOrdersCtx { org: acme.clone() },
        )
        .await?;
    assert_eq!(p1.rows.len(), 2, "a full first page");
    let cursor = p1.cursor.clone().expect("more pages → a cursor");
    let p2 = api
        .recent_orders(
            client::RecentOrdersInput {
                cursor: Some(cursor),
            },
            client::RecentOrdersCtx { org: acme.clone() },
        )
        .await?;
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
    // declared shape — the row still projects while it is soft-deleted.
    let cancelled = api
        .cancel_order(
            client::CancelOrderInput {
                id: placed.id.clone(),
            },
            client::CancelOrderCtx { org: acme.clone() },
        )
        .await?;
    assert_eq!(cancelled.id, placed.id);
    // It is now hidden from ordinary reads (the soft-delete live predicate).
    assert!(
        get(&api, &acme, &placed.id).await?.is_none(),
        "a cancelled order is hidden from a get"
    );
    assert_eq!(
        api.my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: acme.clone() }
        )
        .await?
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
        .await?;
    assert_eq!(restored.id, placed.id);
    assert!(
        get(&api, &acme, &placed.id).await?.is_some(),
        "restored order is readable"
    );
    assert_eq!(
        api.my_orders(
            client::MyOrdersInput,
            client::MyOrdersCtx { org: acme.clone() }
        )
        .await?
        .len(),
        3,
        "back to all three"
    );
    println!("soft-deleted then restored order {}", placed.id);

    // --- 6. handling a typed error ---
    // A malformed cursor is rejected by the engine, and the client surfaces it as a
    // structured `ClientError`. Branch on `kind()` to tell a server-side `Api` failure
    // from a `Transport` / `Decode` one, then read its stable `code()` and HTTP `status()`
    // — this is the pattern a caller uses to handle failures without matching on text.
    let bad_cursor = client::Cursor::from_raw("not-a-real-cursor");
    match api
        .recent_orders(
            client::RecentOrdersInput {
                cursor: Some(bad_cursor),
            },
            client::RecentOrdersCtx { org: acme.clone() },
        )
        .await
    {
        Ok(_) => return Err("a malformed cursor should have been rejected".into()),
        Err(e) => match e.kind() {
            client::ClientErrorKind::Api { status, code } => {
                assert_eq!(*status, 400);
                assert_eq!(code, "bad_cursor");
                assert_eq!(e.code(), "bad_cursor");
                assert_eq!(e.status(), Some(400));
                println!("rejected a malformed cursor: {e}");
            }
            other => return Err(format!("expected a server error, got {other:?}").into()),
        },
    }

    println!("\nend-to-end scenario passed");
    Ok(())
}

/// A whole-dollar amount as a money `Decimal` (scale 2, e.g. `100` -> `100.00`). The
/// generated client types `total` as `rust_decimal::Decimal` and carries it as an exact
/// string on the wire.
fn money(dollars: i64) -> rust_decimal::Decimal {
    rust_decimal::Decimal::new(dollars * 100, 2)
}

/// Place an order for `buyer` at `total`, acting as `org`; return its id.
async fn place(
    api: &client::Client<client::Embedded<'_>>,
    org: &Id<entity::Org>,
    buyer: &Id<entity::User>,
    total: i64,
) -> Result<Id<entity::Order>, client::ClientError> {
    Ok(api
        .place_order(
            client::PlaceOrderInput {
                buyer: buyer.clone(),
                total: money(total),
            },
            client::PlaceOrderCtx { org: org.clone() },
        )
        .await?
        .id)
}

/// Read an order back by id, acting as `org`.
async fn get(
    api: &client::Client<client::Embedded<'_>>,
    org: &Id<entity::Org>,
    id: &Id<entity::Order>,
) -> Result<Option<client::OrderCard>, client::ClientError> {
    api.order_by_id(
        client::OrderByIdInput { id: id.clone() },
        client::OrderByIdCtx { org: org.clone() },
    )
    .await
}
