//! The cancel-safety acceptance gate: a mutation future dropped at **every** await
//! point must leave no trace — the transaction rolls back (or a commit that already
//! completed stands in full, never partially), the pooled connection never carries an
//! open transaction, and the backend serves the next request as if the cancellation
//! never happened.
//!
//! The typestate makes these guarantees by construction (`Db::begin` consumes the
//! connection, `Tx::commit` consumes the transaction, drop = rollback-or-discard);
//! this suite is the systematic proof against a real engine. The mutation path's
//! await points are exactly its driver-seam calls — checkout, begin, each execute,
//! the re-select fetch, commit — so a gate wrapper around a live SQLite backend parks
//! the future at each of them (once just *before* the call, once just *after* it
//! completes), the test drops the future right there, and the invariants are probed
//! through the same single-connection pool the transaction ran on. Await points
//! *inside* one driver call are sqlx's own cancel-safety, reused rather than rebuilt.

#![cfg(feature = "sqlite")]

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures_core::Stream;
use serde_json::json;
use tokio::sync::Notify;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::{
    fetch_all, run_mutation, Backend, Compiled, Db, DbError, DbRead, IdempotencyStore, MemStore,
    NoStore, Request, Row, RowStream, RunError, SeqIdGen, SqlValue, SqliteBackend, Tx,
};
use based_sema::check;

/// A two-write `tx` mutation with a declared-shape re-select — the longest driver-seam
/// op sequence a mutation produces: checkout, begin, INSERT user, INSERT address,
/// re-select fetch, commit.
const SCHEMA: &str = r#"
    @sort(email asc)
    User { id: Id, email: text }
    Address { id: Id, user: User, city: text }
    shape UserCard from User { email }
    query export_users() -> stream UserCard;
    mutation signup(email: text, city: text) -> UserCard {
        tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
        }
    }
"#;

fn compile() -> Compiled {
    let sf = parse_file(SCHEMA, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:#?}");
    Compiled::from_checked(schema, sf.decls, Dialect::Sqlite)
}

/// A fresh **file-backed** SQLite backend (single-connection pool). File-backed on
/// purpose: the invariant permits a cancelled transaction's connection to be recycled
/// *or* discarded, and the data must survive either — an in-memory database would
/// vanish with a discarded connection and mask the distinction.
async fn fresh_backend(c: &Compiled, tag: &str) -> SqliteBackend {
    let path = std::env::temp_dir().join(format!(
        "based-cancel-safety-{}-{tag}.sqlite",
        std::process::id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let backend = SqliteBackend::open(path.to_str().unwrap()).expect("open file sqlite");
    backend
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("generated DDL");
    backend
}

fn signup_request() -> Request {
    Request::new(
        "signup",
        json!({ "email": "a@b.c", "city": "NYC" }),
        json!({}),
    )
}

// ---------- the gate --------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// Park just before the numbered op runs (its effect never happens).
    Before,
    /// Park just after the numbered op completes (its effect happened; the engine
    /// never sees the result).
    After,
}

/// Numbers every driver-seam call in order and parks the mutation future forever at
/// the chosen one, signalling the test — which drops the future right there.
struct Gate {
    next_op: AtomicUsize,
    cancel_at: usize,
    mode: Mode,
    reached: Notify,
}

impl Gate {
    fn cancel_at(op: usize, mode: Mode) -> Arc<Gate> {
        Arc::new(Gate {
            next_op: AtomicUsize::new(0),
            cancel_at: op,
            mode,
            reached: Notify::new(),
        })
    }

    /// A gate that never cancels — counts the ops on the path.
    fn unlimited() -> Arc<Gate> {
        Gate::cancel_at(usize::MAX, Mode::Before)
    }

    fn ops_seen(&self) -> usize {
        self.next_op.load(Ordering::SeqCst)
    }

    fn claim(&self) -> usize {
        self.next_op.fetch_add(1, Ordering::SeqCst)
    }

    /// Park forever, waking the test to drop the future here.
    async fn park(&self) {
        self.reached.notify_one();
        std::future::pending::<()>().await;
    }

    /// Run one driver-seam op under its number, parking at the cancel point.
    async fn point<T>(&self, op: impl std::future::Future<Output = T>) -> T {
        let idx = self.claim();
        if idx == self.cancel_at && self.mode == Mode::Before {
            self.park().await;
        }
        let out = op.await;
        if idx == self.cancel_at && self.mode == Mode::After {
            self.park().await;
        }
        out
    }
}

struct GateBackend {
    gate: Arc<Gate>,
    inner: SqliteBackend,
}

#[async_trait]
impl Backend for GateBackend {
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        let inner = self.gate.point(self.inner.checkout(shard_key)).await?;
        Ok(Box::new(GateDb {
            gate: self.gate.clone(),
            inner,
        }))
    }
}

struct GateDb {
    gate: Arc<Gate>,
    inner: Box<dyn Db>,
}

#[async_trait]
impl DbRead for GateDb {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        Box::pin(GateStream {
            gate: self.gate.clone(),
            idx: None,
            inner: self.inner.fetch(sql, params),
        })
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.gate.point(self.inner.execute(sql, params)).await
    }
}

#[async_trait]
impl Db for GateDb {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        let GateDb { gate, inner } = *self;
        let tx = gate.point(inner.begin()).await?;
        Ok(Box::new(GateTx { gate, inner: tx }))
    }
}

struct GateTx {
    gate: Arc<Gate>,
    inner: Box<dyn Tx>,
}

#[async_trait]
impl DbRead for GateTx {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        Box::pin(GateStream {
            gate: self.gate.clone(),
            idx: None,
            inner: self.inner.fetch(sql, params),
        })
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.gate.point(self.inner.execute(sql, params)).await
    }
}

#[async_trait]
impl Tx for GateTx {
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        let GateTx { gate, inner } = *self;
        gate.point(inner.commit()).await
    }
}

/// A fetch is one numbered op: `Before` parks on the first poll (the query never
/// starts); `After` parks once the stream is exhausted (every row produced, the
/// collected result withheld from the engine).
struct GateStream<'a> {
    gate: Arc<Gate>,
    idx: Option<usize>,
    inner: RowStream<'a>,
}

impl Stream for GateStream<'_> {
    type Item = Result<Row, DbError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let idx = *this.idx.get_or_insert_with(|| this.gate.claim());
        let cancel_here = idx == this.gate.cancel_at;
        if cancel_here && this.gate.mode == Mode::Before {
            this.gate.reached.notify_one();
            return Poll::Pending; // parked; the test drops the future here
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(None) if cancel_here && this.gate.mode == Mode::After => {
                this.gate.reached.notify_one();
                Poll::Pending // parked; the test drops the future here
            }
            other => other,
        }
    }
}

// ---------- the harness ------------------------------------------------------

/// Drive the signup mutation until it either completes (`Some`) or parks at the
/// gate's cancel point — in which case the future is dropped right there (the
/// cancellation under test) and this returns `None`.
async fn run_until_cancelled(
    c: &Compiled,
    backend: &GateBackend,
    store: &dyn IdempotencyStore,
    req: &Request,
) -> Option<Result<serde_json::Value, RunError>> {
    let ids = SeqIdGen::default();
    let mut fut = Box::pin(run_mutation(c, backend, "", &ids, store, req));
    tokio::select! {
        out = &mut fut => Some(out),
        _ = backend.gate.reached.notified() => None, // `fut` drops on return
        _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {
            panic!("mutation neither completed nor reached its cancel point")
        }
    }
}

async fn count(db: &mut dyn Db, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) AS n FROM `{table}`");
    let rows = fetch_all(db.fetch(&sql, &[])).await.expect("count");
    rows[0]["n"].as_i64().expect("integer count")
}

/// The post-cancellation invariants, probed on the same single-connection pool the
/// cancelled transaction ran on.
async fn assert_pool_clean(backend: &SqliteBackend, expect_committed: bool, at: &str) {
    let mut db = Backend::checkout(backend, "")
        .await
        .unwrap_or_else(|e| panic!("{at}: pool must hand out a connection: {e}"));

    // The connection must be in autocommit: a leaked open transaction would make this
    // explicit BEGIN fail ("cannot start a transaction within a transaction").
    db.execute("BEGIN IMMEDIATE", &[])
        .await
        .unwrap_or_else(|e| panic!("{at}: pooled connection carries an open transaction: {e}"));
    db.execute("ROLLBACK", &[]).await.expect("close the probe");

    // All-or-nothing: only a drop after a completed commit leaves rows — all of them.
    let state = (
        count(&mut *db, "user").await,
        count(&mut *db, "address").await,
    );
    let expected = if expect_committed { (1, 1) } else { (0, 0) };
    assert_eq!(
        state, expected,
        "{at}: a cancelled mutation must be all-or-nothing"
    );
}

// ---------- the acceptance gate ----------------------------------------------

#[tokio::test]
async fn dropping_a_mutation_future_at_every_await_point_is_safe() {
    let c = compile();
    let req = signup_request();

    // Discovery: an ungated run counts the driver-seam ops on the path.
    let backend = GateBackend {
        gate: Gate::unlimited(),
        inner: fresh_backend(&c, "discover").await,
    };
    let out = run_mutation(&c, &backend, "", &SeqIdGen::default(), &NoStore, &req)
        .await
        .expect("ungated run");
    assert_eq!(out, json!({ "email": "a@b.c" }));
    let total_ops = backend.gate.ops_seen();
    assert_eq!(
        total_ops, 6,
        "checkout, begin, 2 INSERTs, re-select, commit — the sweep below adapts, \
         but confirm a path change is intentional"
    );

    // The sweep: for every op, drop the future parked just before it ran and just
    // after it completed.
    for op in 0..total_ops {
        for mode in [Mode::Before, Mode::After] {
            let at = format!("cancelled at op {op} ({mode:?})");
            let backend = GateBackend {
                gate: Gate::cancel_at(op, mode),
                inner: fresh_backend(&c, &format!("{op}-{mode:?}")).await,
            };
            let outcome = run_until_cancelled(&c, &backend, &NoStore, &req).await;
            assert!(outcome.is_none(), "{at}: expected to park, but it finished");

            // Writes survive only a drop *after* the commit (the last op) completed —
            // and then in full. Everywhere else the transaction must have vanished.
            let committed = mode == Mode::After && op == total_ops - 1;
            assert_pool_clean(&backend.inner, committed, &at).await;

            // The backend serves the next request untouched: the same mutation,
            // ungated, runs green on the same pool. (Skipped for the committed case —
            // the deterministic test ids would collide with the committed rows.)
            if !committed {
                let redo =
                    run_mutation(&c, &backend.inner, "", &SeqIdGen::default(), &NoStore, &req)
                        .await
                        .unwrap_or_else(|e| {
                            panic!("{at}: pool must serve after a cancellation: {e}")
                        });
                assert_eq!(redo, json!({ "email": "a@b.c" }), "{at}");
            }
        }
    }
}

#[tokio::test]
async fn cancellation_releases_an_idempotency_claim_for_retry() {
    // Drop a *keyed* mutation mid-transaction (parked before its first INSERT). The
    // claim guard must release the key on drop: the client's retry then runs fresh
    // instead of being rejected as in-flight forever.
    let c = compile();
    let store = MemStore::new();
    let req = signup_request().with_idempotency_key(Some("key-1".into()));

    let backend = GateBackend {
        gate: Gate::cancel_at(2, Mode::Before),
        inner: fresh_backend(&c, "keyed").await,
    };
    let outcome = run_until_cancelled(&c, &backend, &store, &req).await;
    assert!(outcome.is_none(), "expected to park at the first INSERT");

    let redo = run_mutation(&c, &backend.inner, "", &SeqIdGen::default(), &store, &req)
        .await
        .expect("a retry after a cancelled keyed mutation must run, not conflict");
    assert_eq!(redo, json!({ "email": "a@b.c" }));
}

#[tokio::test]
async fn dropping_a_stream_mid_pass_leaves_the_pool_clean() {
    // The streaming twin of the mutation sweep: drop a `-> stream` query's row stream
    // after its first row (the caller cancelled mid-pass) and prove the invariants on
    // the same single-connection pool — the connection comes back, carries no open
    // transaction, and serves the next mutation as if nothing happened.
    use futures_util::StreamExt;

    let c = compile();
    let backend = fresh_backend(&c, "stream-drop").await;

    // Two committed users so the pass has more rows than we consume.
    let seed_ids = SeqIdGen::default();
    for (email, city) in [("a@b.c", "NYC"), ("b@b.c", "SF")] {
        let req = Request::new("signup", json!({ "email": email, "city": city }), json!({}));
        based_runtime::run_mutation(&c, &backend, "", &seed_ids, &NoStore, &req)
            .await
            .expect("seed mutation");
    }

    let req = Request::new("export_users", json!({}), json!({}));
    let db = Backend::checkout(&backend, "").await.expect("checkout");
    let mut stream = based_runtime::run_query_stream(&c, db, &req).expect("stream starts");
    let first = stream.next().await.expect("has a first row");
    assert_eq!(first.expect("row decodes"), json!({ "email": "a@b.c" }));
    // The cancellation under test: the stream (and the connection it owns) drops here.
    drop(stream);

    // The pooled connection is back and in autocommit — a leaked open transaction (or
    // a stranded read) would make this explicit BEGIN fail on the single-connection pool.
    let mut db = Backend::checkout(&backend, "")
        .await
        .expect("pool must hand the connection back after the drop");
    db.execute("BEGIN IMMEDIATE", &[])
        .await
        .expect("pooled connection must be in autocommit after a dropped stream");
    db.execute("ROLLBACK", &[]).await.expect("close the probe");
    assert_eq!(count(&mut *db, "user").await, 2, "reads mutate nothing");
    drop(db);

    // And the same pool serves the next mutation green.
    let req = Request::new(
        "signup",
        json!({ "email": "c@b.c", "city": "LA" }),
        json!({}),
    );
    let redo =
        based_runtime::run_mutation(&c, &backend, "", &SeqIdGen::new("post"), &NoStore, &req)
            .await
            .expect("pool must serve after a cancelled stream");
    assert_eq!(redo, json!({ "email": "c@b.c" }));
}
