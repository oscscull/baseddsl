//! Sema tests: parse a snippet, run `check`, and assert on the diagnostics and
//! the resolved schema. Snippets are whole (multi-decl) schemas so cross-model
//! resolution (relations, inverses, return types) is exercised end to end.

use based_ast::{FileId, Verb};
use based_diagnostics::{Diagnostic, Severity};
use based_parser::parse_file;
use based_sema::{check, CheckedSchema, MemberKind, SoftMode};

fn analyze(src: &str) -> (CheckedSchema, Vec<Diagnostic>) {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    check(&sf.decls)
}

fn codes(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code).collect()
}

fn errors(diags: &[Diagnostic]) -> Vec<&str> {
    diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code)
        .collect()
}

fn assert_clean(src: &str) {
    let (_, diags) = analyze(src);
    assert!(
        diags.is_empty(),
        "expected no diagnostics, got: {:?}",
        codes(&diags)
    );
}

#[path = "check/aggregates.rs"]
mod aggregates;
#[path = "check/atomic_updates.rs"]
mod atomic_updates;
#[path = "check/callables.rs"]
mod callables;
#[path = "check/computed_fields.rs"]
mod computed_fields;
#[path = "check/context.rs"]
mod context;
#[path = "check/custom_joins.rs"]
mod custom_joins;
#[path = "check/decorators.rs"]
mod decorators;
#[path = "check/destructive_mutations.rs"]
mod destructive_mutations;
#[path = "check/distinct.rs"]
mod distinct;
#[path = "check/duplicates.rs"]
mod duplicates;
#[path = "check/enums.rs"]
mod enums;
#[path = "check/filters.rs"]
mod filters;
#[path = "check/generated_columns.rs"]
mod generated_columns;
#[path = "check/happy_path.rs"]
mod happy_path;
#[path = "check/indexes.rs"]
mod indexes;
#[path = "check/inverses.rs"]
mod inverses;
#[path = "check/keyless_models.rs"]
mod keyless_models;
#[path = "check/keys.rs"]
mod keys;
#[path = "check/lints.rs"]
mod lints;
#[path = "check/multi_scope.rs"]
mod multi_scope;
#[path = "check/numbers.rs"]
mod numbers;
#[path = "check/opaque_columns.rs"]
mod opaque_columns;
#[path = "check/operand_typing.rs"]
mod operand_typing;
#[path = "check/optional_context.rs"]
mod optional_context;
#[path = "check/optional_filters.rs"]
mod optional_filters;
#[path = "check/raw_queries.rs"]
mod raw_queries;
#[path = "check/renames.rs"]
mod renames;
#[path = "check/resolution_errors.rs"]
mod resolution_errors;
#[path = "check/schemas.rs"]
mod schemas;
#[path = "check/scopes.rs"]
mod scopes;
#[path = "check/shapes.rs"]
mod shapes;
#[path = "check/sort_decorators.rs"]
mod sort_decorators;
#[path = "check/tx_bindings.rs"]
mod tx_bindings;
#[path = "check/upserts.rs"]
mod upserts;
