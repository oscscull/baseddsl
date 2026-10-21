//! Test fixtures against a real SQLite database: the engine inserts any model's row through
//! the structured create path, outside every route, scope and guard.

use serde_json::{json, Value};

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::{Backend, Compiled, Db, DbError, Engine, SeqIdGen, TxOptions};
use crate::driver::SqliteBackend;
use based_sema::check;

const SCHEMA: &str = r#"
scope Tenant (org: Org = $ctx.org)
enum Tier { free, pro }

Org {
  id: Id
  name: text
  slug: text (unique)
  tier: Tier
  active: bool (default true)
}

@created(created_at)
@scope Tenant
Note {
  id: Id
  org: Org
  title: text
  body: text?
  stars: int (default 3)
  created_at: timestamp
}

Tag { id: serial, label: text }

shape NoteRow from Note { id, title }
query notes() -> NoteRow[] unscoped("test reads every tenant") { list Note; }
"#;

fn compiled() -> Compiled {
    let sf = parse_file(SCHEMA, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| (d.code, d.message.clone()))
        .collect();
    assert!(errors.is_empty(), "unexpected sema errors: {errors:?}");
    Compiled::from_checked(schema, sf.decls, Dialect::Sqlite)
}

async fn engine() -> Engine {
    let compiled = compiled();
    let backend = SqliteBackend::in_memory().unwrap();
    backend
        .execute_batch(&sql::ddl(&compiled.schema, Dialect::Sqlite))
        .await
        .unwrap();
    Engine::new(compiled, backend, SeqIdGen::default())
}

async fn insert(engine: &Engine, model: &str, row: Value) -> Value {
    let response = engine.insert_fixture(model, row, "").await;
    assert_eq!(response.status, 200, "{model} fixture: {:?}", response.body);
    response.body
}

async fn org(engine: &Engine, slug: &str) -> Value {
    insert(
        engine,
        "Org",
        json!({ "name": "Acme", "slug": slug, "tier": "pro" }),
    )
    .await
}

async fn note_titles(engine: &Engine) -> Vec<Value> {
    let response = engine.call("/q/notes", json!({}), json!({})).await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    response
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["title"].clone())
        .collect()
}

#[tokio::test]
async fn fixture_returns_the_whole_stored_row() {
    let engine = engine().await;
    let org = org(&engine, "acme").await;
    assert_eq!(org["id"], "id-0", "the engine minted the id");
    assert_eq!(org["tier"], "pro");
    assert!(
        org["active"] == json!(1) || org["active"] == json!(true),
        "unset defaulted column takes the database default: {org:?}"
    );

    // No `$ctx`: the scope column is an ordinary field and `@created` still stamps.
    let note = insert(
        &engine,
        "Note",
        json!({ "org": { "id": org["id"] }, "title": "First" }),
    )
    .await;
    assert_eq!(note["org"], org["id"]);
    assert_eq!(note["title"], "First");
    assert_eq!(note["body"], Value::Null, "unset optional column is NULL");
    assert_eq!(note["stars"], 3);
    assert!(note["created_at"].is_string(), "{note:?}");
}

#[tokio::test]
async fn fixture_sets_engine_managed_columns_when_given() {
    let engine = engine().await;
    let org = org(&engine, "acme").await;
    let note = insert(
        &engine,
        "Note",
        json!({
            "id": "chosen",
            "org": { "id": org["id"] },
            "title": "Dated",
            "created_at": "2000-01-01 00:00:00",
            "stars": 5,
        }),
    )
    .await;
    assert_eq!(note["id"], "chosen");
    assert_eq!(note["created_at"], "2000-01-01 00:00:00");
    assert_eq!(note["stars"], 5);

    let tag = insert(&engine, "Tag", json!({ "label": "serial" })).await;
    assert!(tag["id"].is_number(), "a serial id is read back: {tag:?}");
}

#[tokio::test]
async fn fixture_errors_name_the_problem() {
    let engine = engine().await;
    let unknown = engine.insert_fixture("Nope", json!({}), "").await;
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.body["error"]["code"], "unknown_fixture");

    let field = engine
        .insert_fixture(
            "Org",
            json!({ "name": "A", "slug": "a", "tier": "free", "x": 1 }),
            "",
        )
        .await;
    assert_eq!(field.status, 400);
    assert_eq!(field.body["error"]["code"], "bad_fixture");

    let missing = engine
        .insert_fixture("Org", json!({ "name": "A" }), "")
        .await;
    assert_eq!(missing.status, 400, "{:?}", missing.body);
    let message = missing.body["error"]["message"].as_str().unwrap();
    assert!(message.contains("slug"), "{message}");
}

#[tokio::test]
async fn fixture_ops_are_never_routed() {
    let engine = engine().await;
    let org = org(&engine, "acme").await;
    for route in ["/m/based_fixture", "/q/based_fixture"] {
        let response = engine
            .call(route, json!({ "fixture": { "name": "x" } }), json!({}))
            .await;
        assert_eq!(response.status, 404, "{route}: {:?}", response.body);
    }
    assert!(engine.compiled().mutations.is_empty());
    assert_eq!(org["slug"], "acme");
}

#[tokio::test]
async fn fixture_runs_inside_an_engine_transaction() {
    let engine = engine().await;
    let org = org(&engine, "acme").await;
    let row = json!({ "org": { "id": org["id"] }, "title": "Rolled back" });

    let txn = engine.begin(TxOptions::new()).await.unwrap();
    let inside = txn.transport().insert_fixture("Note", row).await;
    assert_eq!(inside.status, 200, "{:?}", inside.body);
    txn.rollback().await.unwrap();
    assert!(note_titles(&engine).await.is_empty());

    let txn = engine.begin(TxOptions::new()).await.unwrap();
    let row = json!({ "org": { "id": org["id"] }, "title": "Committed" });
    assert_eq!(
        txn.transport().insert_fixture("Note", row).await.status,
        200
    );
    txn.commit().await.unwrap();
    assert_eq!(note_titles(&engine).await, vec![json!("Committed")]);
}

#[tokio::test]
async fn fixture_runs_on_an_adopted_transaction() {
    crate::driver::adopted_fixture_inserts(compiled()).await;
}

/// Records the shard key of every checkout, over a real SQLite backend.
struct ShardLog {
    inner: SqliteBackend,
    keys: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Backend for ShardLog {
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        self.keys.lock().unwrap().push(shard_key.to_string());
        self.inner.checkout(shard_key).await
    }
}

#[tokio::test]
async fn fixture_inserts_on_the_requested_shard() {
    let compiled = compiled();
    let inner = SqliteBackend::in_memory().unwrap();
    inner
        .execute_batch(&sql::ddl(&compiled.schema, Dialect::Sqlite))
        .await
        .unwrap();
    let log = std::sync::Arc::new(ShardLog {
        inner,
        keys: std::sync::Mutex::new(Vec::new()),
    });
    let engine = Engine::new(compiled, SharedLog(log.clone()), SeqIdGen::default());
    let row = json!({ "name": "Sharded", "slug": "sharded", "tier": "free" });
    let response = engine.insert_fixture("Org", row, "tenant-7").await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    assert_eq!(*log.keys.lock().unwrap(), vec!["tenant-7".to_string()]);
}

/// Lets the test keep a handle on the log the engine owns.
struct SharedLog(std::sync::Arc<ShardLog>);

#[async_trait::async_trait]
impl Backend for SharedLog {
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        self.0.checkout(shard_key).await
    }
}
