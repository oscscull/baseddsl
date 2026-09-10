//! End-to-end integration against a **real** engine (SQLite), no mock.
//!
//! Every other runtime test drives the plan → run → shape path against a `MockDb` that
//! returns canned rows — so it proves the *binding* is right but never that the emitted
//! SQL actually executes. This test closes that gap: it loads the real commerce schema
//! (`Compiled::load`), seeds an in-memory SQLite database, and dispatches real requests
//! through `serve::dispatch` against the concrete `SqliteDb`/`SqliteBackend`. What runs is
//! the *verbatim* codegen-lowered SQL (`based gen sql`), bound positionally by the runtime
//! — so a passing test means the whole engine works against a genuine database, and that
//! the `Db`/`Backend`/`ping` seams are real, not just compile-verified.
//!
//! It needs no infra (bundled in-memory SQLite), so it runs in CI like any unit test.
//!
//! SQLite accepts the runtime's DML as-is (backtick identifiers, `= TRUE`, `IS NULL`,
//! joins, `LIMIT`, positional `?`). The **DDL** is also generated for SQLite —
//! the tables here are created from `based gen sql`'s SQLite output (`sql::ddl` with
//! `Dialect::Sqlite`) run against the loaded schema, so the whole `based gen sql` artifact
//! (DDL *and* DML) is now proven to execute, not just the query text.

#![cfg(feature = "sqlite")]

use std::path::PathBuf;

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::run::Backend;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

/// Load the real commerce example — the same front end (discover → parse → check) +
/// codegen lowering the CLI uses, so the SQL executed here is `based gen sql`'s output.
fn commerce() -> Compiled {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/examples/commerce")
        .canonicalize()
        .expect("commerce example dir");
    Compiled::load(&root).unwrap_or_else(|e| panic!("commerce did not load: {e:?}"))
}

/// Seed an in-memory SQLite database: create every commerce table from the *generated*
/// SQLite DDL (`based gen sql` with `Dialect::Sqlite`), then insert a couple of rows.
/// Running the real DDL — not a hand-shaped copy — means this test now exercises the whole
/// `based gen sql` artifact end to end: the DDL creates the schema the DML then reads/writes.
async fn seeded_backend(c: &Compiled) -> SqliteBackend {
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("generated SQLite DDL failed to execute: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`, `slug`) VALUES ('org-1', 'Acme', 'acme');
            INSERT INTO `user` (`id`, `email`, `name`) VALUES ('user-1', 'a@x.com', 'Ada');
            INSERT INTO `order` (`id`, `org_id`, `placed_by_id`, `status`, `total`)
                VALUES ('order-1', 'org-1', 'user-1', 'paid', '500.00');
            "#,
        )
        .await
        .expect("seed fixtures");
    backend
}

/// Run one request through the real dispatch core against the live backend — the exact
/// path `based serve` uses, minus the socket (dispatch checks its own connection out).
async fn call(
    compiled: &Compiled,
    backend: &SqliteBackend,
    method: &str,
    path: &str,
    args: serde_json::Value,
    ctx: serde_json::Value,
) -> based_runtime::WireResponse {
    let ids = SeqIdGen::default();
    dispatch(
        compiled,
        backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        method,
        path,
        args,
        ctx,
        None,
    )
    .await
}

fn compile_sqlite(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    Compiled::from_checked(schema, sf.decls, Dialect::Sqlite)
}

#[path = "sqlite_integration/adopt.rs"]
mod adopt;
#[path = "sqlite_integration/aggregates.rs"]
mod aggregates;
#[path = "sqlite_integration/bulk_deletes.rs"]
mod bulk_deletes;
#[path = "sqlite_integration/commerce.rs"]
mod commerce;
#[path = "sqlite_integration/custom_joins.rs"]
mod custom_joins;
#[path = "sqlite_integration/enums_guards_raw.rs"]
mod enums_guards_raw;
#[path = "sqlite_integration/mutations.rs"]
mod mutations;
#[path = "sqlite_integration/nested_to_many.rs"]
mod nested_to_many;
#[path = "sqlite_integration/nested_to_one.rs"]
mod nested_to_one;
#[path = "sqlite_integration/nullable_pagination.rs"]
mod nullable_pagination;
#[path = "sqlite_integration/pagination.rs"]
mod pagination;
#[path = "sqlite_integration/relations_and_locks.rs"]
mod relations_and_locks;
#[path = "sqlite_integration/scalar_types.rs"]
mod scalar_types;
#[path = "sqlite_integration/scopes.rs"]
mod scopes;
#[path = "sqlite_integration/tx_bindings.rs"]
mod tx_bindings;
#[path = "sqlite_integration/upserts_and_ddl.rs"]
mod upserts_and_ddl;
#[path = "sqlite_integration/write_recovery.rs"]
mod write_recovery;
