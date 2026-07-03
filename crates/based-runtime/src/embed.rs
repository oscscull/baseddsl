//! In-process embedding (Tier 1) — run callables with **no socket**.
//!
//! [`Engine`] is the library twin of `based serve`: it owns a [`Compiled`] schema, one
//! database connection ([`Db`]), and an id generator ([`IdGen`]), and runs a callable
//! straight through [`crate::serve::dispatch`] — the *same* wire core the HTTP listener
//! uses, minus the socket. So an embedded call and an HTTP call take the identical
//! plan → run → shape path and yield the identical [`WireResponse`] (principle 4 — one
//! engine, two front doors).
//!
//! ## Why in-process
//! The per-call cost is the DB round-trip (0.2–5 ms, D20); over the wire it is also the
//! loopback TCP + HTTP framing. Dropping the socket removes that framing while keeping
//! the *same typed generated client* — a Rust app gets one binary (no sidecar), lower
//! and steadier latency, and `MockDb`-backed end-to-end tests. It is also the path
//! toward app-owned transactions (composing several callables over one connection),
//! which stateless HTTP RPC cannot express.
//!
//! ## Wiring the generated client
//! The generated client (`based gen client`) is generic over a `Transport` trait it
//! *defines itself*, so — by the orphan rule — the `impl Transport` bridging it to an
//! `Engine` lives in your crate, next to the generated module. It is a few lines: serialize
//! the typed input to JSON args, call [`Engine::call`], then decode the `200` body into the
//! output type (a non-`200` becomes the client's `ClientError`). `$ctx` is supplied
//! straight in — no header dance (auth.md/D7 still holds: the *app*, not the caller, sets
//! it). A worked example lives in `tests/embed.rs`:
//!
//! ```ignore
//! struct InProcess<'a> { engine: &'a Engine, ctx: serde_json::Value }
//!
//! impl client::Transport for InProcess<'_> {
//!     fn call<I: Serialize, O: DeserializeOwned>(&self, route: &str, input: &I)
//!         -> Result<O, client::ClientError>
//!     {
//!         let args = serde_json::to_value(input).map_err(|e| client::ClientError(e.to_string()))?;
//!         let resp = self.engine.call(route, args, self.ctx.clone());
//!         if resp.status == 200 {
//!             serde_json::from_value(resp.body).map_err(|e| client::ClientError(e.to_string()))
//!         } else {
//!             let msg = resp.body["error"]["message"].as_str().unwrap_or("call failed");
//!             Err(client::ClientError(msg.to_string()))
//!         }
//!     }
//! }
//! ```
//!
//! ## Concurrency
//! `Engine` holds its one connection behind a [`RefCell`], so a call needs only `&self`
//! (which is what backs the `&self` `Transport::call`). That makes it single-threaded by
//! design — one embedded connection, used from one thread at a time. A multi-threaded or
//! pooled embed routes through the [`crate::run::Backend`] seam instead (check out a
//! connection per request, build a short-lived `Engine` around it) — the same seam
//! `based serve` uses.

use std::cell::RefCell;

use crate::id::IdGen;
use crate::load::Compiled;
use crate::run::Db;
use crate::serve::{dispatch, WireResponse};

/// An in-process engine: a loaded schema + one database connection + an id generator,
/// ready to run callables directly. Build one with [`Engine::new`] (from a
/// [`Compiled`], via [`Compiled::load`] or [`Compiled::from_checked`]) and call it with
/// a route, JSON args, and the request `$ctx`.
pub struct Engine {
    compiled: Compiled,
    // Interior mutability: `dispatch` needs `&mut` on the connection and id-gen, but a
    // call takes `&self` so it can back the generated `Transport` (also `&self`). One
    // connection ⇒ one thread at a time — a pooled embed uses `Backend` instead.
    db: RefCell<Box<dyn Db>>,
    id_gen: RefCell<Box<dyn IdGen>>,
}

impl Engine {
    /// Build an engine over a compiled schema, a database connection, and an id
    /// generator. For a `MockDb`-backed test pass [`crate::id::SeqIdGen`]; a real embed
    /// passes its own [`Db`] (e.g. the caller's existing pool checkout) and a uuid
    /// generator (D1).
    pub fn new(compiled: Compiled, db: impl Db + 'static, id_gen: impl IdGen + 'static) -> Engine {
        Engine {
            compiled,
            db: RefCell::new(Box::new(db)),
            id_gen: RefCell::new(Box::new(id_gen)),
        }
    }

    /// Run one callable and return the wire response — the same status + JSON body the
    /// HTTP edge would produce. `route` is `/q/<name>` or `/m/<name>` (the generated
    /// client supplies the constant), `args` is the JSON argument object, and `ctx` is
    /// the request `$ctx` the app derived from its auth layer (never from the caller —
    /// auth.md/D7). The method is always `POST`: the closed RPC surface has no other
    /// verb (calling.md).
    pub fn call(
        &self,
        route: &str,
        args: serde_json::Value,
        ctx: serde_json::Value,
    ) -> WireResponse {
        dispatch(
            &self.compiled,
            self.db.borrow_mut().as_mut(),
            self.id_gen.borrow_mut().as_mut(),
            "POST",
            route,
            args,
            ctx,
        )
    }

    /// The compiled schema this engine serves (its lowered queries/mutations + resolved
    /// schema), for callers that want to introspect what routes exist.
    pub fn compiled(&self) -> &Compiled {
        &self.compiled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::SeqIdGen;
    use crate::run::{MockDb, Row};
    use serde_json::json;

    fn row(v: serde_json::Value) -> Row {
        v.as_object().cloned().unwrap()
    }

    const SCHEMA: &str = r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, status: text, total: int }
        shape OrderCard from Order { status, total }

        query order_by_id(id) -> OrderCard;
        mutation place_order(org: Id, status, total: int) -> OrderCard {
            create Order { org = $org, status = $status, total = $total };
        }
    "#;

    fn compiled() -> Compiled {
        use based_ast::FileId;
        let sf = based_parser::parse_file(SCHEMA, FileId(0)).expect("parse");
        let (schema, diags) = based_sema::check(&sf.decls);
        assert!(!diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error));
        Compiled::from_checked(schema, sf.decls)
    }

    /// A read call over the engine returns the same shaped `200` a `dispatch` would.
    #[test]
    fn engine_runs_a_query() {
        let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
        let engine = Engine::new(compiled(), db, SeqIdGen::default());

        let resp = engine.call("/q/order_by_id", json!({ "id": "o-1" }), json!({}));
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, json!({ "status": "paid", "total": 42 }));
    }

    /// A write call runs the transaction and returns the created id — no socket.
    #[test]
    fn engine_runs_a_mutation() {
        let db = MockDb::new(vec![]);
        let engine = Engine::new(compiled(), db, SeqIdGen::default());

        let resp = engine.call(
            "/m/place_order",
            json!({ "org": "o-1", "status": "open", "total": 7 }),
            json!({}),
        );
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, json!({ "id": "id-0" }));
    }

    /// Boundary failures map to the same statuses the wire uses (unknown route → 404).
    #[test]
    fn engine_reports_boundary_errors() {
        let engine = Engine::new(compiled(), MockDb::new(vec![]), SeqIdGen::default());
        let resp = engine.call("/q/nope", json!({}), json!({}));
        assert_eq!(resp.status, 404);
        assert_eq!(resp.body["error"]["code"], "unknown_query");
    }
}
