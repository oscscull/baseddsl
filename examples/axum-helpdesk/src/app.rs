//! Service wiring: the app's own Postgres pool, the engine over it, and the
//! close policy behind the schema's `guard caller_can_close`.

use std::path::PathBuf;

use based_runtime::guard::{GuardRequest, GuardVerdict, Guards};
use based_runtime::id::UuidGen;
use based_runtime::{Compiled, Engine, PgRouter};

use crate::client;

/// Shared service state: the engine every handler's typed client runs through.
/// Cheap to clone into axum state (the engine itself is a shared handle).
#[derive(Clone)]
pub struct App {
    engine: Engine,
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
        // build until an implementation is registered.
        let guards = Guards::new().register("caller_can_close", |req| caller_can_close(req));
        let engine = Engine::with_guards(compiled, PgRouter::from_pool(pool), UuidGen, guards)
            .expect("every declared guard is registered");
        App { engine }
    }

    /// A typed client over the embedded engine — what every handler calls.
    pub fn api(&self) -> client::Client<client::Embedded<'_>> {
        client::embedded(&self.engine)
    }
}

/// The close policy: a ticket must be resolved before anyone closes it. The engine
/// guarantees this runs before the write; the decision is app code — but the *read*
/// it decides on goes back through the schema's own `ticket` query over `req.engine()`,
/// so the workspace scope and the soft-delete filter are the ones the schema declares. 
/// A check that cannot decide denies — fail closed.
async fn caller_can_close(req: GuardRequest) -> GuardVerdict {
    let (Ok(input), Ok(ctx)) = (
        serde_json::from_value::<client::TicketInput>(req.args.clone()),
        serde_json::from_value::<client::TicketCtx>(req.ctx.clone()),
    ) else {
        return GuardVerdict::deny("close requires a ticket id and a workspace");
    };
    match client::embedded(req.engine()).ticket(input, ctx).await {
        Ok(Some(t)) if t.status == client::Status::Resolved => GuardVerdict::Allow,
        Ok(Some(_)) => GuardVerdict::deny("only a resolved ticket can be closed"),
        Ok(None) => GuardVerdict::deny("no such ticket in this workspace"),
        Err(_) => GuardVerdict::deny("could not verify the ticket"),
    }
}
