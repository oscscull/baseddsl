//! In-process embedding — run callables with no socket.
//!
//! [`Engine`] is the library twin of `based serve`: it owns a [`Compiled`] schema, a
//! [`Backend`] (the connection source), and an id generator ([`IdGen`]), and runs a
//! callable straight through [`crate::serve::dispatch`] — the same wire core the HTTP
//! listener uses, minus the socket. So an embedded call and an HTTP call take the
//! identical plan → run → shape path and yield the identical [`WireResponse`].
//!
//! Dropping the socket removes the loopback TCP + HTTP framing while keeping the same
//! typed generated client — one binary (no sidecar), lower and steadier latency, and
//! `MockDb`-backed end-to-end tests.
//!
//! ## Wiring the generated client
//! The generated client (`based gen client`) is generic over a `Transport` trait it
//! defines itself, so — by the orphan rule — a library-side `impl Transport for Engine`
//! in this crate is forbidden. Instead `based gen client` emits the bridge when asked
//! (`ClientOptions::embedded`): the generated module carries an `Embedded` transport over
//! `Engine` plus an `embedded(&engine)` constructor, so wiring a client is one call and
//! zero bridge code:
//!
//! ```ignore
//! let api = client::embedded(&engine);              // no Transport impl to write
//! let out = api.place_order(input, ctx).await?;      // typed, in-process, no socket
//! ```
//!
//! The emitted bridge serializes the typed input and the typed `$ctx` to JSON, calls
//! [`Engine::call`], then decodes the `200` body into the output type (a non-`200` becomes
//! the client's `ClientError`). `$ctx` is a typed method argument supplied straight in —
//! the app, not the caller, sets it; a public callable passes `()`, which the bridge maps
//! to an empty context bag.
//!
//! ## Concurrency
//! `Engine` is `Send + Sync`: every call checks a connection out of the [`Backend`]
//! for its own duration, so it is safe to `Arc` an engine into shared state (e.g. an
//! axum router) and call it from any number of tasks — concurrency is bounded by the
//! backend's pool, exactly like the HTTP edge.

use std::sync::Arc;

use crate::guard::{GuardSetupError, Guards};
use crate::id::IdGen;
use crate::idempotency::MemStore;
use crate::load::Compiled;
use crate::run::Backend;
use crate::serve::{dispatch, dispatch_stream, resolve_shard_key, route_target, WireResponse};

/// An in-process engine: a loaded schema + a connection [`Backend`] + an id generator,
/// ready to run callables directly. Build one with [`Engine::new`] (from a
/// [`Compiled`], via [`Compiled::load`] or [`Compiled::from_checked`]) and call it with
/// a route, JSON args, and the request `$ctx`.
pub struct Engine {
    compiled: Compiled,
    backend: Arc<dyn Backend>,
    // The id generator is engine-owned mutable state; an async-aware lock so a call
    // holding it across dispatch's awaits stays `Send`.
    id_gen: tokio::sync::Mutex<Box<dyn IdGen>>,
    // An in-process idempotency store for keyed mutation retries via
    // [`Engine::call_with_key`]. `MemStore` is correct for a single embedded instance;
    // [`Engine::call`] (no key) never consults it.
    store: MemStore,
    // The registered host guard implementations (auth.md Handle 3); construction
    // guarantees every guard the schema declares is present.
    guards: Guards,
}

impl Engine {
    /// Build an engine over a compiled schema, a connection backend, and an id
    /// generator. For a [`crate::run::MockDb`]-backed test pass [`crate::id::SeqIdGen`];
    /// a real embed passes its own [`Backend`] (e.g. a driver router over its pool) and
    /// a uuid generator.
    ///
    /// # Panics
    /// If the schema declares a `guard` — a guard is a host function this constructor
    /// cannot know about, and a declared check must never silently not run. Build a
    /// guarded schema's engine with [`Engine::with_guards`] instead.
    pub fn new(
        compiled: Compiled,
        backend: impl Backend + 'static,
        id_gen: impl IdGen + 'static,
    ) -> Engine {
        match Engine::with_guards(compiled, backend, id_gen, Guards::new()) {
            Ok(engine) => engine,
            Err(e) => panic!("{e} (use Engine::with_guards)"),
        }
    }

    /// Like [`Engine::new`], registering the host guard implementations for the
    /// schema's `guard` declarations. Fails — at build, not at request time — when a
    /// declared guard has no registered implementation.
    pub fn with_guards(
        compiled: Compiled,
        backend: impl Backend + 'static,
        id_gen: impl IdGen + 'static,
        guards: Guards,
    ) -> Result<Engine, GuardSetupError> {
        let missing = guards.missing_for(&compiled);
        if !missing.is_empty() {
            return Err(GuardSetupError { missing });
        }
        Ok(Engine {
            compiled,
            backend: Arc::new(backend),
            id_gen: tokio::sync::Mutex::new(Box::new(id_gen)),
            store: MemStore::new(),
            guards,
        })
    }

    /// Run one callable and return the wire response — the same status + JSON body the
    /// HTTP edge would produce. `route` is `/q/<name>` or `/m/<name>` (the generated
    /// client supplies the constant), `args` is the JSON argument object, and `ctx` is
    /// the request `$ctx` the app derived from its auth layer (never from the caller). The
    /// method is always `POST`: the closed RPC surface has no other verb.
    pub async fn call(
        &self,
        route: &str,
        args: serde_json::Value,
        ctx: serde_json::Value,
    ) -> WireResponse {
        self.call_with_key(route, args, ctx, None).await
    }

    /// Like [`Engine::call`], with a mutation idempotency key. A retry of a
    /// `/m/<name>` mutation with the same non-empty `idem_key` replays the first
    /// attempt's response instead of writing again (queries ignore the key). This is the
    /// in-process twin of the HTTP edge's `Idempotency-Key` header — supplied straight in,
    /// no header dance.
    pub async fn call_with_key(
        &self,
        route: &str,
        args: serde_json::Value,
        ctx: serde_json::Value,
        idem_key: Option<String>,
    ) -> WireResponse {
        // The same schema-derived shard routing as the HTTP edge (an unroutable path
        // keys to "" and 404s in dispatch).
        let shard_key = route_target(route)
            .map(|(is_mutation, name)| {
                resolve_shard_key(&self.compiled, is_mutation, name, &ctx, None)
            })
            .unwrap_or_default();
        let mut id_gen = self.id_gen.lock().await;
        dispatch(
            &self.compiled,
            &*self.backend,
            &shard_key,
            id_gen.as_mut(),
            &self.store,
            &self.guards,
            "POST",
            route,
            args,
            ctx,
            idem_key,
        )
        .await
    }

    /// Run one `-> stream` query and yield its shaped rows in-process — the library
    /// twin of the HTTP edge's NDJSON body, minus the framing (no socket, no lines).
    /// A failure before the first row (unknown route, bad args, missing `$ctx`, a
    /// checkout fault) is the same [`WireResponse`] the wire would send; a mid-stream
    /// database failure is the stream's last item, and dropping the stream cancels the
    /// read and releases its connection.
    pub async fn call_stream(
        &self,
        route: &str,
        args: serde_json::Value,
        ctx: serde_json::Value,
    ) -> Result<crate::run::ShapedStream, WireResponse> {
        let shard_key = route_target(route)
            .map(|(is_mutation, name)| {
                resolve_shard_key(&self.compiled, is_mutation, name, &ctx, None)
            })
            .unwrap_or_default();
        dispatch_stream(
            &self.compiled,
            &*self.backend,
            &shard_key,
            "POST",
            route,
            args,
            ctx,
        )
        .await
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
        Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
    }

    /// A read call over the engine returns the same shaped `200` a `dispatch` would.
    #[tokio::test]
    async fn engine_runs_a_query() {
        let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
        let engine = Engine::new(compiled(), db, SeqIdGen::default());

        let resp = engine
            .call("/q/order_by_id", json!({ "id": "o-1" }), json!({}))
            .await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, json!({ "status": "paid", "total": 42 }));
    }

    /// A write call runs the transaction and returns the created row in its declared
    /// shape, read back inside the tx — no socket.
    #[tokio::test]
    async fn engine_runs_a_mutation() {
        let db = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
        let engine = Engine::new(compiled(), db.clone(), SeqIdGen::default());

        let resp = engine
            .call(
                "/m/place_order",
                json!({ "org": "o-1", "status": "open", "total": 7 }),
                json!({}),
            )
            .await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, json!({ "status": "open", "total": 7 }));
        assert_eq!(db.tx_log(), vec!["begin", "commit"]);
    }

    /// Boundary failures map to the same statuses the wire uses (unknown route → 404).
    #[tokio::test]
    async fn engine_reports_boundary_errors() {
        let engine = Engine::new(compiled(), MockDb::new(vec![]), SeqIdGen::default());
        let resp = engine.call("/q/nope", json!({}), json!({})).await;
        assert_eq!(resp.status, 404);
        assert_eq!(resp.body["error"]["code"], "unknown_query");
    }
}
