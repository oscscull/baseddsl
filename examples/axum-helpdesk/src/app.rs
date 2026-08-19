//! Service wiring: the app's own Postgres pool, the engine over it, and the
//! close policy behind the schema's `guard caller_can_close`.

use std::path::PathBuf;

use based_runtime::guard::{GuardRequest, GuardVerdict, Guards};
use based_runtime::id::UuidGen;
use based_runtime::{Compiled, Engine, PgRouter};

use crate::client;

use crate::client::{entity, Id};

/// Shared service state: the engine every handler's typed client runs through, plus the
/// app's own `sqlx` pool — the one the interop demo (`resolve_with_audit`) opens its own
/// transactions on. Cheap to clone into axum state (both are shared handles).
#[derive(Clone)]
pub struct App {
    engine: Engine,
    pool: sqlx::PgPool,
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

        // An app-owned table the engine knows nothing about — the interop demo writes to it
        // raw, in the same transaction as its baseddsl status update, to prove both land
        // atomically. Created here so a fresh (reset) database has it.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS audit_log (\
               id text PRIMARY KEY, ticket_id text NOT NULL, action text NOT NULL, \
               at timestamptz NOT NULL DEFAULT now())",
        )
        .execute(&pool)
        .await
        .expect("create audit_log");

        // The schema declares `guard caller_can_close`, so the engine refuses to
        // build until an implementation is registered.
        let guards = Guards::new().register("caller_can_close", |req| caller_can_close(req));
        // The engine runs over a router on the app's pool; the app keeps the pool too, so
        // `resolve_with_audit` can open its *own* transactions on the same connections and
        // adopt them into the engine.
        let engine = Engine::with_guards(compiled, PgRouter::from_pool(pool.clone()), UuidGen, guards)
            .expect("every declared guard is registered");
        App { engine, pool }
    }

    /// A typed client over the embedded engine — what every handler calls.
    pub fn api(&self) -> client::Client<client::Embedded<'_>> {
        client::embedded(&self.engine)
    }

    /// **Interop demo — bring-your-own transaction (`adopt`, transactions.md rung 3).**
    /// Resolve a ticket AND write an app-owned `audit_log` row in **one caller-owned
    /// transaction**, so the raw app write and the baseddsl write commit atomically — or,
    /// when `abort`, roll back together.
    ///
    /// The app opens the transaction on its *own* pool, does its raw audit `INSERT`, then
    /// **adopts** that transaction into the engine (`client::adopt_postgres`) and runs
    /// baseddsl calls on it: a `for update` locking read (the adopted client is `TxBound`, so
    /// the lock is held to the caller's boundary) followed by the status update. `adopt`
    /// never begins/commits/rolls back — the app owns the boundary and commits itself.
    pub async fn resolve_with_audit(
        &self,
        ticket: Id<entity::Ticket>,
        org: Id<entity::Org>,
        abort: bool,
    ) -> Result<client::TicketRow, InteropError> {
        let mut tx = self.pool.begin().await?;

        // (1) a raw, app-owned write on the caller's own transaction — not baseddsl.
        sqlx::query("INSERT INTO audit_log (id, ticket_id, action) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(ticket.as_str())
            .bind(if abort { "resolve-aborted" } else { "resolved" })
            .execute(&mut *tx)
            .await?;

        // (2) baseddsl work through `adopt`, on the SAME transaction. The adopted client is
        // scoped in a block so it drops — releasing its borrow of `tx` — before commit.
        let row = {
            let api = client::adopt_postgres(&self.engine, &mut tx);
            // Lock the ticket row for this transaction (no-op-safe elsewhere; a real row lock
            // on Postgres), proving `for update` works through an adopted client.
            api.ticket_for_update(
                client::TicketForUpdateInput { id: ticket.clone() },
                client::TicketForUpdateCtx { org: org.clone() },
            )
            .await?
            .ok_or(InteropError::NotFound)?;
            // Then the baseddsl status update — same transaction, same scope.
            api.set_status(
                client::SetStatusInput {
                    id: ticket,
                    status: client::Status::Resolved,
                },
                client::SetStatusCtx { org },
            )
            .await?
        };

        if abort {
            // The caller owns the boundary: rolling back discards BOTH writes together.
            tx.rollback().await?;
            return Err(InteropError::Aborted);
        }
        tx.commit().await?; // the audit row and the status update land atomically
        Ok(row)
    }

    /// Count the app-owned audit rows for a ticket — a raw read on the app's pool, so the
    /// smoke can prove the interop write actually committed (or didn't).
    pub async fn audit_count(&self, ticket_id: &str) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT count(*) FROM audit_log WHERE ticket_id = $1")
            .bind(ticket_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }
}

/// A failure of the `adopt` interop flow (`resolve_with_audit`): a database error opening or
/// committing the caller's transaction, a baseddsl client error, the ticket not being visible
/// in scope, or the deliberate demo abort.
#[derive(Debug)]
pub enum InteropError {
    Db(sqlx::Error),
    Client(client::ClientError),
    NotFound,
    Aborted,
}

impl From<sqlx::Error> for InteropError {
    fn from(e: sqlx::Error) -> Self {
        InteropError::Db(e)
    }
}

impl From<client::ClientError> for InteropError {
    fn from(e: client::ClientError) -> Self {
        InteropError::Client(e)
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
