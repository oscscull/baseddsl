//! DML (query -> SELECT) codegen tests. Parse + check a whole-schema snippet, then
//! assert on the generated SELECT text. The headline assertions are the soft-delete
//! injection (root `WHERE` + every join `ON`) and the sort/pagination cascade.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

fn gen(src: &str) -> String {
    gen_for(src, Dialect::MariaDb)
}

fn gen_pg(src: &str) -> String {
    gen_for(src, Dialect::Postgres)
}

fn gen_for(src: &str, dialect: Dialect) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    // These snippets exercise SELECT lowering, not index completeness — a query that
    // scans an unindexed column (`E0260`) still lowers to correct SQL, and the index
    // requirement is covered authoritatively in based-sema's tests + conformance.
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    sql::dml::dml(&schema, &sf.decls, dialect)
}

fn query_section<'a>(ddl: &'a str, name: &str) -> &'a str {
    let head = format!("-- query {name}\n");
    let start = ddl.find(&head).expect("query section present");
    let rest = &ddl[start..];
    match rest[head.len()..].find("\n-- query ") {
        Some(i) => &rest[..head.len() + i],
        None => rest,
    }
}

#[path = "dml/aggregates.rs"]
mod aggregates;
#[path = "dml/computed_fields.rs"]
mod computed_fields;
#[path = "dml/custom_joins.rs"]
mod custom_joins;
#[path = "dml/distinct.rs"]
mod distinct;
#[path = "dml/enum_filters.rs"]
mod enum_filters;
#[path = "dml/flattening.rs"]
mod flattening;
#[path = "dml/locking.rs"]
mod locking;
#[path = "dml/nested_to_many.rs"]
mod nested_to_many;
#[path = "dml/nested_to_one.rs"]
mod nested_to_one;
#[path = "dml/pagination.rs"]
mod pagination;
#[path = "dml/postgres.rs"]
mod postgres;
#[path = "dml/projections.rs"]
mod projections;
#[path = "dml/query_options.rs"]
mod query_options;
#[path = "dml/raw_queries.rs"]
mod raw_queries;
#[path = "dml/scopes.rs"]
mod scopes;
