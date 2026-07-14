//! Service wiring: the app's own Postgres pool, the engine over it, and the
//! close policy behind the schema's `guard caller_can_close`.

use std::path::PathBuf;
use std::sync::Arc;

use based_runtime::guard::{GuardRequest, GuardVerdict, Guards};
use based_runtime::id::UuidGen;
use based_runtime::{Compiled, Engine, PgRouter};

use crate::client;

/// Shared service state: the engine every handler's typed client runs through.
/// Cheap to clone into axum state (the engine itself is shared).
#[derive(Clone)]
pub struct App {
    engine: Arc<Engine>,
}

impl App {
    /// Wire the desk against a Postgres URL. The pool is the app's own sqlx pool —
    /// its settings govern — and the engine runs over it (`PgRouter::from_pool`),
    /// so the app's queries and the engine's writes share one set of connections.
    pub async fn connect(database_url: &str) -> App {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(database_url)
            .await
            .unwrap_or_else(|e| panic!("connect to Postgres at {database_url}: {e}"));

        // The same front end `based check` runs; the tables already exist
        // (`based migrate apply`), so the service issues no DDL.
        let compiled = Compiled::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("schema checks clean");

        // The schema declares `guard caller_can_close`, so the engine refuses to
        // build until an implementation is registered. The policy is host code on
        // the app's own pool — the same pool the engine runs over.
        let guards = Guards::new().register("caller_can_close", {
            let db = pool.clone();
            move |req| caller_can_close(db.clone(), req)
        });
        let engine = Engine::with_guards(compiled, PgRouter::from_pool(pool), UuidGen, guards)
            .expect("every declared guard is registered");
        App {
            engine: Arc::new(engine),
        }
    }

    /// A typed client over the embedded engine — what every handler calls.
    pub fn api(&self) -> client::Client<client::Embedded<'_>> {
        client::embedded(&self.engine)
    }
}

/// The close policy: a ticket must be resolved — and visible in the caller's
/// workspace — before anyone closes it. The engine guarantees this runs before the
/// write; the decision itself is app code, reading current state off the app's own
/// pool (its one SQL line owns its own tombstone filter). A check that cannot
/// decide denies — fail closed.
async fn caller_can_close(db: sqlx::PgPool, req: GuardRequest) -> GuardVerdict {
    let (Some(id), Some(org)) = (req.args["id"].as_str(), req.ctx["org"].as_str()) else {
        return GuardVerdict::deny("close requires a ticket id and a workspace");
    };
    let status: Result<Option<String>, _> = sqlx::query_scalar(
        "select status from ticket \
         where id = $1::uuid and org_id = $2::uuid and deleted_at is null",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(&db)
    .await;
    match status {
        Ok(Some(s)) if s == "resolved" => GuardVerdict::Allow,
        Ok(Some(_)) => GuardVerdict::deny("only a resolved ticket can be closed"),
        Ok(None) => GuardVerdict::deny("no such ticket in this workspace"),
        Err(_) => GuardVerdict::deny("could not verify the ticket"),
    }
}
