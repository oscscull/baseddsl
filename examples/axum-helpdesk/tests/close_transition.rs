//! Exercise the actual helpdesk schema and preflight policy against Postgres.
//! Run after migrations with DATABASE_URL pointing to a disposable test database.

use axum_helpdesk::client::{self, fixtures};
use axum_helpdesk::close_policy;
use based_runtime::id::UuidGen;
use based_runtime::{
    AdoptedPg, AdoptedTransport, Compiled, Engine, GuardVerdict, Guards, PgRouter,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::{timeout, Duration};

struct Desk {
    pool: PgPool,
    org: String,
    user: String,
    ticket: String,
}

impl Desk {
    async fn seed() -> Option<Self> {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping helpdesk transition test: set DATABASE_URL to a migrated test DB");
            return None;
        };
        let pool = PgPool::connect(&url).await.unwrap();
        let engine = engine(&pool, policy());
        let api = client::embedded(&engine);
        let org = fixtures::org().name("Transition test").insert(&api).await.unwrap();
        let user = fixtures::user(&org)
            .name("Agent")
            .role(client::Role::Agent)
            .insert(&api)
            .await
            .unwrap();
        let ticket = fixtures::ticket(&org, &user)
            .subject("Transition")
            .status(client::Status::Resolved)
            .insert(&api)
            .await
            .unwrap();
        let (org, user, ticket) = (
            org.id.into_raw(),
            user.id.into_raw(),
            ticket.id.into_raw(),
        );
        Some(Self {
            pool,
            org,
            user,
            ticket,
        })
    }

    fn engine(&self, guards: Guards) -> Engine {
        engine(&self.pool, guards)
    }

    fn ctx(&self) -> Value {
        json!({ "org": self.org })
    }
    fn args(&self) -> Value {
        json!({ "id": self.ticket })
    }

    async fn status(&self) -> String {
        sqlx::query_scalar("SELECT status FROM ticket WHERE id = $1::uuid")
            .bind(uuid::Uuid::parse_str(&self.ticket).unwrap())
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn set_status(&self, status: &str) {
        sqlx::query("UPDATE ticket SET status = $1 WHERE id = $2::uuid")
            .bind(status)
            .bind(uuid::Uuid::parse_str(&self.ticket).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn cleanup(self) {
        for (query, id) in [
            ("DELETE FROM ticket WHERE id = $1::uuid", self.ticket),
            ("DELETE FROM \"user\" WHERE id = $1::uuid", self.user),
            ("DELETE FROM org WHERE id = $1::uuid", self.org),
        ] {
            sqlx::query(query)
                .bind(uuid::Uuid::parse_str(&id).unwrap())
                .execute(&self.pool)
                .await
                .unwrap();
        }
    }
}

fn engine(pool: &PgPool, guards: Guards) -> Engine {
    let compiled = Compiled::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    Engine::with_guards(compiled, PgRouter::from_pool(pool.clone()), UuidGen, guards).unwrap()
}

fn policy() -> Guards {
    Guards::new().register("caller_can_close", close_policy::caller_can_close)
}

#[tokio::test]
async fn resolved_close_succeeds_and_preflight_denies_invalid_rows() {
    let Some(desk) = Desk::seed().await else {
        return;
    };
    let engine = desk.engine(policy());
    let closed = engine
        .call("/m/close_ticket", desk.args(), desk.ctx())
        .await;
    assert_eq!(closed.status, 200, "{:?}", closed.body);
    assert_eq!(closed.body["status"], "closed");
    desk.set_status("open").await;
    for (args, ctx) in [
        (desk.args(), desk.ctx()),
        (
            json!({ "id": uuid::Uuid::new_v4().to_string() }),
            desk.ctx(),
        ),
        (
            desk.args(),
            json!({ "org": uuid::Uuid::new_v4().to_string() }),
        ),
    ] {
        let response = engine.call("/m/close_ticket", args, ctx).await;
        assert_eq!(response.status, 403, "{:?}", response.body);
        assert_eq!(response.body["error"]["code"], "guard_denied");
    }
    assert_eq!(desk.status().await, "open");
    desk.cleanup().await;
}

async fn racing_close(adopted: bool) {
    let Some(desk) = Desk::seed().await else {
        return;
    };
    let approved = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let guards = Guards::new().register("caller_can_close", {
        let approved = approved.clone();
        let resume = resume.clone();
        move |req| {
            let approved = approved.clone();
            let resume = resume.clone();
            async move {
                let verdict = close_policy::caller_can_close(req).await;
                assert_eq!(verdict, GuardVerdict::Allow);
                approved.notify_one();
                resume.notified().await;
                verdict
            }
        }
    });
    let engine = desk.engine(guards);
    let (args, ctx, pool) = (desk.args(), desk.ctx(), desk.pool.clone());
    let close = tokio::spawn(async move {
        if !adopted {
            return engine.call("/m/close_ticket", args, ctx).await;
        }
        let mut tx = pool.begin().await.unwrap();
        let response = {
            let transport = AdoptedTransport::new(engine, AdoptedPg::new(&mut tx));
            transport.dispatch("/m/close_ticket", args, ctx).await
        };
        // Even committing the caller's transaction must not close the reopened row.
        tx.commit().await.unwrap();
        response
    });
    timeout(Duration::from_secs(5), approved.notified())
        .await
        .unwrap();
    desk.set_status("open").await; // committed by another connection after approval
    resume.notify_one();
    let response = timeout(Duration::from_secs(5), close)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 404, "{:?}", response.body);
    assert_eq!(response.body["error"]["code"], "not_found");
    assert_eq!(desk.status().await, "open");
    desk.cleanup().await;
}

#[tokio::test]
async fn concurrent_reopen_after_guard_approval_cannot_close() {
    racing_close(false).await;
}

#[tokio::test]
async fn adopted_transaction_also_enforces_the_conditional_transition() {
    racing_close(true).await;
}
