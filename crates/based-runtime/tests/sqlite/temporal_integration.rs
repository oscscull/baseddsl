//! Timestamps and dates on SQLite (issue #181): a bound value is validated and stored in
//! one canonical UTC text form, and engine-written times (`now()`, `@created`) use the
//! same form, so text comparison, `order`, and deletes by age follow time order.

use serde_json::json;

use crate::driver::SqliteBackend;
use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen};
use based_sema::check;

const SCHEMA: &str = r#"
@sort(at desc)
Activity { id: Id, text: text, at: timestamp, @index at }
shape ActivityRow from Activity { text, at }
query recent_activity() -> ActivityRow[] { list Activity page (50); }
query activity_pages() -> ActivityRow[] { list Activity page (1); }
mutation add_activity(text, at: timestamp) -> ok { create Activity { text = $text, at = $at }; }
mutation expire_activity(before: timestamp) -> ok { delete Activity where (at < $before); }

@created(made)
RunRequest { id: Id, label: text, requested_at: timestamp (default now()), made: timestamp, on: date? }
shape RunRow from RunRequest { label, requested_at, made, on }
query runs() -> RunRow[] { list RunRequest order (label); }
mutation add_run(label, on: date?) -> ok { create RunRequest { label = $label, on = $on }; }
"#;

struct Db {
    compiled: Compiled,
    backend: SqliteBackend,
    ids: SeqIdGen,
}

async fn db() -> Db {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean: {diags:?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed to execute: {e:?}\n{ddl}"));
    Db {
        compiled,
        backend,
        ids: SeqIdGen::default(),
    }
}

async fn call(db: &Db, path: &str, args: serde_json::Value) -> based_runtime::WireResponse {
    dispatch(
        &db.compiled,
        &db.backend,
        "",
        &db.ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        path,
        args,
        json!({}),
        None,
    )
    .await
}

async fn add(db: &Db, text: &str, at: &str) {
    let r = call(db, "/m/add_activity", json!({ "text": text, "at": at })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
}

/// One string field of every row in a page envelope.
fn field(body: &serde_json::Value, key: &str) -> Vec<String> {
    body["rows"]
        .as_array()
        .expect("page rows")
        .iter()
        .map(|r| r[key].as_str().expect("string field").to_string())
        .collect()
}

#[tokio::test]
async fn an_unparseable_timestamp_is_a_bad_arg() {
    let db = db().await;
    let r = call(&db, "/m/add_activity", json!({ "text": "x", "at": "not a time" })).await;
    assert_eq!(r.status, 400, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "bad_arg", "{:?}", r.body);
    let rows = call(&db, "/q/recent_activity", json!({})).await;
    assert!(field(&rows.body, "text").is_empty(), "{:?}", rows.body);
}

#[tokio::test]
async fn mixed_input_forms_store_canonical_and_sort_by_time() {
    let db = db().await;
    add(&db, "eight", "2026-10-21T08:00:00Z").await;
    add(&db, "half past", "2026-10-21 08:30:00").await;
    add(&db, "nine", "2026-10-21T11:00:00+02:00").await;
    let r = call(&db, "/q/recent_activity", json!({})).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(field(&r.body, "text"), ["nine", "half past", "eight"]);
    assert_eq!(
        field(&r.body, "at"),
        [
            "2026-10-21T09:00:00.000000Z",
            "2026-10-21T08:30:00.000000Z",
            "2026-10-21T08:00:00.000000Z",
        ]
    );
}

#[tokio::test]
async fn delete_by_age_compares_as_time() {
    let db = db().await;
    add(&db, "eight", "2026-10-21T08:00:00Z").await;
    add(&db, "half past", "2026-10-21 08:30:00").await;
    let r = call(
        &db,
        "/m/expire_activity",
        json!({ "before": "2026-10-21T08:15:00Z" }),
    )
    .await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    let rows = call(&db, "/q/recent_activity", json!({})).await;
    assert_eq!(field(&rows.body, "text"), ["half past"]);
}

/// A keyset cursor over mixed input forms walks rows in time order.
#[tokio::test]
async fn keyset_pages_follow_time_order() {
    let db = db().await;
    add(&db, "eight", "2026-10-21T08:00:00Z").await;
    add(&db, "half past", "2026-10-21 08:30:00").await;
    let first = call(&db, "/q/activity_pages", json!({})).await;
    assert_eq!(field(&first.body, "text"), ["half past"], "{:?}", first.body);
    let cursor = first.body["cursor"].clone();
    let second = call(&db, "/q/activity_pages", json!({ "cursor": cursor })).await;
    assert_eq!(field(&second.body, "text"), ["eight"], "{:?}", second.body);
}

/// `default now()` and `@created` write the same canonical form a bind does.
#[tokio::test]
async fn engine_written_times_use_the_canonical_form() {
    let db = db().await;
    let r = call(&db, "/m/add_run", json!({ "label": "a", "on": "2026-1-5" })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    let rows = call(&db, "/q/runs", json!({})).await;
    let row = &rows.body[0];
    assert!(row.is_object(), "{:?}", rows.body);
    for key in ["requested_at", "made"] {
        let at = row[key].as_str().expect("timestamp");
        assert_eq!(at.len(), "2026-10-21T08:00:00.000000Z".len(), "{at}");
        assert_eq!(&at[10..11], "T", "{at}");
        assert!(at.ends_with('Z'), "{at}");
    }
    assert_eq!(row["on"], "2026-01-05");
}

#[tokio::test]
async fn an_unparseable_date_is_a_bad_arg() {
    let db = db().await;
    let r = call(&db, "/m/add_run", json!({ "label": "a", "on": "21/10/2026" })).await;
    assert_eq!(r.status, 400, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "bad_arg", "{:?}", r.body);
}
