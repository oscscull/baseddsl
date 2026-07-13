//! Seed the helpdesk with demo data — two tenants, agents and requesters, tickets
//! in every state — through the typed client's own mutations. No raw SQL.
//!
//! Run it once against a freshly migrated database (`based migrate apply`, then
//! `cargo run --bin seed`). It prints the demo bearer tokens the service's auth
//! middleware resolves into per-request context.

use axum_helpdesk::client::{self, Id, Priority, Role, Status};
use based_runtime::guard::{GuardVerdict, Guards};
use based_runtime::id::UuidGen;
use based_runtime::shard::PoolConfig;
use based_runtime::{Compiled, Engine, PgRouter};
use rust_decimal::Decimal;
use std::path::PathBuf;

use client::entity;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL (see .env)");

    // Same front end `based check` runs; the tables already exist (`based migrate
    // apply` created them), so this program issues no DDL.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compiled = Compiled::load(&manifest).expect("schema checks clean");

    let router = PgRouter::single(&url, PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to Postgres at {url}: {e:?}"));

    // The schema declares `guard caller_can_close`, so the engine won't build
    // without an implementation. Seeding acts as the desk owner, so it always
    // allows; the service registers the real policy.
    let guards = Guards::new().register("caller_can_close", |_req| async { GuardVerdict::Allow });
    let engine =
        Engine::with_guards(compiled, router, UuidGen, guards).expect("every guard registered");
    let api = client::embedded(&engine);

    // ---- tenants -----------------------------------------------------------
    let acme = org(&api, "Acme Support", "acme").await;
    let globex = org(&api, "Globex", "globex").await;

    // ---- people ------------------------------------------------------------
    let mara = user(
        &api,
        &acme,
        "Mara",
        "mara@acme.test",
        Role::Agent,
        Some(money("95.00")),
    )
    .await;
    let noah = user(
        &api,
        &acme,
        "Noah",
        "noah@acme.test",
        Role::Agent,
        Some(money("80.00")),
    )
    .await;
    let ada = user(
        &api,
        &acme,
        "Ada",
        "ada@customer.test",
        Role::Requester,
        None,
    )
    .await;
    let bea = user(
        &api,
        &acme,
        "Bea",
        "bea@customer.test",
        Role::Requester,
        None,
    )
    .await;
    let gus = user(
        &api,
        &globex,
        "Gus",
        "gus@globex.test",
        Role::Agent,
        Some(money("70.00")),
    )
    .await;
    let greta = user(
        &api,
        &globex,
        "Greta",
        "greta@partner.test",
        Role::Requester,
        None,
    )
    .await;

    // ---- sessions (the demo bearer tokens) ----------------------------------
    let tokens = [
        ("tok-acme-mara", &acme, &mara, "acme", "agent", "Mara"),
        ("tok-acme-noah", &acme, &noah, "acme", "agent", "Noah"),
        ("tok-acme-ada", &acme, &ada, "acme", "requester", "Ada"),
        ("tok-acme-bea", &acme, &bea, "acme", "requester", "Bea"),
        ("tok-globex-gus", &globex, &gus, "globex", "agent", "Gus"),
        (
            "tok-globex-greta",
            &globex,
            &greta,
            "globex",
            "requester",
            "Greta",
        ),
    ];
    for (token, org_id, user_id, _, _, _) in &tokens {
        api.start_session(
            client::StartSessionInput {
                org: (*org_id).clone(),
                user: (*user_id).clone(),
                token: (*token).to_string(),
            },
            (),
        )
        .await
        .expect("start_session");
    }

    // ---- Acme's desk ---------------------------------------------------------
    // Requesters open tickets (a ticket + its first comment land in one tx).
    let t1 = open(
        &api,
        &acme,
        &ada,
        "Password reset loop",
        "I reset my password and get bounced straight back to the reset page.",
        Some(Priority::High),
    )
    .await;
    let t2 = open(
        &api,
        &acme,
        &ada,
        "Invoice PDF won't download",
        "The download button spins forever on invoice #4211.",
        None,
    )
    .await;
    let t3 = open(
        &api,
        &acme,
        &bea,
        "Add SSO for our team",
        "We'd like SAML SSO for the whole workspace.",
        Some(Priority::Low),
    )
    .await;
    let t4 = open(
        &api,
        &acme,
        &bea,
        "Data export is empty",
        "Last night's CSV export finished but the file has only headers.",
        Some(Priority::Urgent),
    )
    .await;
    let t5 = open(
        &api,
        &acme,
        &ada,
        "Password reset loop (again)",
        "Same loop as my earlier ticket, still locked out.",
        Some(Priority::High),
    )
    .await;
    let t6 = open(
        &api,
        &acme,
        &bea,
        "You have won a prize",
        "Click here to claim it.",
        None,
    )
    .await;

    // Agents triage.
    assign(&api, &acme, &t1, &mara).await;
    assign(&api, &acme, &t4, &noah).await;
    api.set_status(
        client::SetStatusInput {
            id: t2.clone(),
            status: Status::Waiting,
        },
        client::SetStatusCtx { org: acme.clone() },
    )
    .await
    .expect("set_status");
    api.tag_ticket(
        client::TagTicketInput {
            id: t4.clone(),
            tags: serde_json::json!(["vip", "export"]),
        },
        client::TagTicketCtx { org: acme.clone() },
    )
    .await
    .expect("tag_ticket");
    api.mark_duplicate(
        client::MarkDuplicateInput {
            id: t5.clone(),
            of: t1.clone(),
        },
        client::MarkDuplicateCtx { org: acme.clone() },
    )
    .await
    .expect("mark_duplicate");
    api.close_ticket(
        client::CloseTicketInput { id: t3.clone() },
        client::CloseTicketCtx { org: acme.clone() },
    )
    .await
    .expect("close_ticket (guard allows the seeder)");
    api.archive_ticket(
        client::ArchiveTicketInput { id: t6 },
        client::ArchiveTicketCtx { org: acme.clone() },
    )
    .await
    .expect("archive_ticket");

    // The conversation on t1 (comments display oldest-first).
    comment(
        &api,
        &acme,
        &mara,
        &t1,
        "Looking into it — can you say which browser?",
    )
    .await;
    comment(&api, &acme, &ada, &t1, "Firefox, but Safari does the same.").await;

    // Billable work on t1, logged out of chronological order on purpose: the
    // detail view must come back sorted by `logged_at`, not by insertion.
    log(
        &api,
        &mara,
        &t1,
        1.5,
        money("142.50"),
        "repro + traced the redirect",
        "2026-07-08 14:00:00+00",
    )
    .await;
    log(
        &api,
        &mara,
        &t1,
        0.5,
        money("47.50"),
        "first triage",
        "2026-07-06 09:30:00+00",
    )
    .await;
    log(
        &api,
        &mara,
        &t1,
        2.0,
        money("190.00"),
        "patched the token check",
        "2026-07-07 11:15:00+00",
    )
    .await;
    log(
        &api,
        &noah,
        &t4,
        1.0,
        money("80.00"),
        "re-ran the export with tracing",
        "2026-07-09 10:00:00+00",
    )
    .await;

    // A private draft only Mara (in Acme) can read back.
    api.save_draft(
        client::SaveDraftInput {
            ticket: t1.clone(),
            body: "Suspect the reset token is consumed by the email previewer. Verify before replying.".into(),
        },
        client::SaveDraftCtx { org: acme.clone(), user: mara.clone() },
    )
    .await
    .expect("save_draft");

    // ---- Globex's desk (so cross-tenant scoping is observable) ---------------
    let g1 = open(
        &api,
        &globex,
        &greta,
        "Webhook retries flooding us",
        "Every failed delivery retries once a second.",
        Some(Priority::High),
    )
    .await;
    assign(&api, &globex, &g1, &gus).await;

    // ---- sanity: read the seed back the way the service will -----------------
    let mine = api
        .my_tickets(
            client::MyTicketsInput,
            client::MyTicketsCtx { user: ada.clone() },
        )
        .await
        .expect("my_tickets");
    assert_eq!(mine.len(), 3, "Ada sees exactly her own live tickets");

    let detail = api
        .ticket(
            client::TicketInput { id: t1.clone() },
            client::TicketCtx { org: acme.clone() },
        )
        .await
        .expect("ticket")
        .expect("t1 exists");
    assert_eq!(detail.comments.len(), 3, "opening comment + two replies");
    let logged: Vec<&str> = detail
        .time_entries
        .iter()
        .map(|e| e.logged_at.as_str())
        .collect();
    let mut sorted = logged.clone();
    sorted.sort();
    assert_eq!(logged, sorted, "time entries come back in logged_at order");
    assert_eq!(
        detail.duplicates.len(),
        1,
        "t5 is linked as a duplicate of t1"
    );

    let queue = api
        .queue(
            client::QueueInput,
            client::QueueCtx {
                org: acme.clone(),
                user: mara.clone(),
            },
        )
        .await
        .expect("queue");
    assert!(
        queue.iter().any(|t| t.id == t1),
        "t1 is high priority and Mara's"
    );

    println!("seeded 2 orgs, 6 users, 7 tickets\n");
    println!("demo bearer tokens:");
    for (token, _, _, org_slug, role, name) in &tokens {
        println!("  {org_slug:<7} {role:<10} {name:<6} {token}");
    }
}

// ---- small helpers over the typed client -----------------------------------

/// Money as the exact `Decimal` the wire carries (`"95.00"` stays `95.00`).
fn money(amount: &str) -> Decimal {
    amount.parse().expect("a decimal literal")
}

type Api<'e> = client::Client<client::Embedded<'e>>;

async fn org(api: &Api<'_>, name: &str, slug: &str) -> Id<entity::Org> {
    api.create_org(
        client::CreateOrgInput {
            name: name.into(),
            slug: slug.into(),
        },
        (),
    )
    .await
    .expect("create_org")
    .id
}

async fn user(
    api: &Api<'_>,
    org: &Id<entity::Org>,
    name: &str,
    email: &str,
    role: Role,
    rate: Option<Decimal>,
) -> Id<entity::User> {
    api.create_user(
        client::CreateUserInput {
            org: org.clone(),
            name: name.into(),
            email: email.into(),
            role,
            rate,
        },
        (),
    )
    .await
    .expect("create_user")
    .id
}

async fn open(
    api: &Api<'_>,
    org: &Id<entity::Org>,
    requester: &Id<entity::User>,
    subject: &str,
    body: &str,
    priority: Option<Priority>,
) -> Id<entity::Ticket> {
    api.open_ticket(
        client::OpenTicketInput {
            subject: subject.into(),
            body: body.into(),
            priority,
        },
        client::OpenTicketCtx {
            org: org.clone(),
            user: requester.clone(),
        },
    )
    .await
    .expect("open_ticket")
    .id
}

async fn assign(
    api: &Api<'_>,
    org: &Id<entity::Org>,
    ticket: &Id<entity::Ticket>,
    agent: &Id<entity::User>,
) {
    api.assign_ticket(
        client::AssignTicketInput {
            id: ticket.clone(),
            agent: agent.clone(),
        },
        client::AssignTicketCtx { org: org.clone() },
    )
    .await
    .expect("assign_ticket");
}

async fn comment(
    api: &Api<'_>,
    org: &Id<entity::Org>,
    author: &Id<entity::User>,
    ticket: &Id<entity::Ticket>,
    body: &str,
) {
    api.add_comment(
        client::AddCommentInput {
            ticket: ticket.clone(),
            body: body.into(),
        },
        client::AddCommentCtx {
            org: org.clone(),
            user: author.clone(),
        },
    )
    .await
    .expect("add_comment");
}

async fn log(
    api: &Api<'_>,
    agent: &Id<entity::User>,
    ticket: &Id<entity::Ticket>,
    hours: f64,
    amount: Decimal,
    note: &str,
    logged_at: &str,
) {
    api.log_time(
        client::LogTimeInput {
            ticket: ticket.clone(),
            hours,
            amount,
            note: note.into(),
            logged_at: logged_at.into(),
        },
        client::LogTimeCtx {
            user: agent.clone(),
        },
    )
    .await
    .expect("log_time");
}
