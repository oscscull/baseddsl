//! FK referential-action tests: the structural checks (`E0290`–`E0294`, convention-free)
//! and the manifest-dependent divergence-reason rule (`E0295`/`W0110`), exercised in both
//! `foreign_keys` directions.

use based_ast::FileId;
use based_diagnostics::{Diagnostic, Severity};
use based_parser::parse_file;
use based_sema::{check, check_foreign_keys, CheckedSchema, ForeignKeys};

fn analyze(src: &str) -> (CheckedSchema, Vec<Diagnostic>) {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    check(&sf.decls)
}

fn codes(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code).collect()
}

fn structural_codes(src: &str) -> Vec<String> {
    let (_, d) = analyze(src);
    d.iter().map(|x| x.code.to_string()).collect()
}

/// Codes from the manifest-dependent divergence pass under a given convention.
fn fk_codes(src: &str, fks: ForeignKeys) -> Vec<String> {
    let (schema, d) = analyze(src);
    assert!(
        d.iter().all(|x| x.severity != Severity::Error),
        "base check had errors: {:?}",
        codes(&d)
    );
    check_foreign_keys(&schema, fks)
        .iter()
        .map(|x| x.code.to_string())
        .collect()
}

const ORG: &str = "Org { id: Id  name: text }\n";

// ---------- structural checks (convention-free) ----------------------------

#[test]
fn fk_on_forward_relation_is_structurally_clean() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}");
    // No structural error (the reason rule is a separate pass).
    let (_, d) = analyze(&src);
    assert!(
        d.iter().all(|x| x.severity != Severity::Error),
        "{:?}",
        codes(&d)
    );
}

#[test]
fn fk_on_scalar_is_e0290() {
    let src = format!("{ORG}Order {{ id: Id  name: text @fk }}");
    assert!(structural_codes(&src).contains(&"E0290".to_string()));
}

#[test]
fn no_fk_on_inverse_edge_is_e0290() {
    let src = format!(
        "{ORG}Order {{ id: Id  org: Org  items: OrderItem[] @no_fk }}\n\
         OrderItem {{ id: Id  order: Order }}"
    );
    assert!(structural_codes(&src).contains(&"E0290".to_string()));
}

#[test]
fn fk_on_custom_join_relation_is_e0291() {
    let src = format!("{ORG}Order {{ id: Id  org: Org (on: order.org_ref = org.legacy_key) @fk }}");
    assert!(structural_codes(&src).contains(&"E0291".to_string()));
}

#[test]
fn fk_and_no_fk_on_same_edge_is_e0292() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @fk @no_fk }}");
    assert!(structural_codes(&src).contains(&"E0292".to_string()));
}

#[test]
fn set_null_on_required_relation_is_e0293() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @fk(on_delete: set_null) }}");
    assert!(structural_codes(&src).contains(&"E0293".to_string()));
}

#[test]
fn set_null_on_optional_relation_is_ok() {
    let src = format!("{ORG}Order {{ id: Id  org: Org? @fk(on_delete: set_null) }}");
    let (_, d) = analyze(&src);
    assert!(
        d.iter().all(|x| x.severity != Severity::Error),
        "{:?}",
        codes(&d)
    );
}

#[test]
fn unknown_action_is_e0294() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @fk(on_delete: explode) }}");
    assert!(structural_codes(&src).contains(&"E0294".to_string()));
}

// ---------- divergence-reason rule (manifest-dependent) --------------------

#[test]
fn fk_under_none_without_reason_is_e0295() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}");
    assert_eq!(fk_codes(&src, ForeignKeys::None), vec!["E0295"]);
}

#[test]
fn fk_under_none_with_reason_is_clean() {
    let src = format!(
        "{ORG}Order {{ id: Id  org: Org @fk(\"orders die with their org\", on_delete: cascade) }}"
    );
    assert!(fk_codes(&src, ForeignKeys::None).is_empty());
}

#[test]
fn no_fk_under_none_is_redundant_w0110() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @no_fk }}");
    assert_eq!(fk_codes(&src, ForeignKeys::None), vec!["W0110"]);
}

#[test]
fn no_fk_under_all_without_reason_is_e0295() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @no_fk }}");
    assert_eq!(fk_codes(&src, ForeignKeys::All), vec!["E0295"]);
}

#[test]
fn no_fk_under_all_with_reason_is_clean() {
    let src =
        format!("{ORG}Order {{ id: Id  org: Org @no_fk(\"legacy table, FKs banned at scale\") }}");
    assert!(fk_codes(&src, ForeignKeys::All).is_empty());
}

#[test]
fn bare_fk_under_all_is_redundant_w0110() {
    let src = format!("{ORG}Order {{ id: Id  org: Org @fk }}");
    assert_eq!(fk_codes(&src, ForeignKeys::All), vec!["W0110"]);
}

#[test]
fn fk_with_actions_under_all_is_clean() {
    // Refining a present FK's action never needs a reason.
    let src = format!("{ORG}Order {{ id: Id  org: Org @fk(on_delete: cascade) }}");
    assert!(fk_codes(&src, ForeignKeys::All).is_empty());
}

#[test]
fn model_no_fk_under_all_without_reason_is_e0295() {
    let src = format!("{ORG}@no_fk Order {{ id: Id  org: Org }}");
    assert_eq!(fk_codes(&src, ForeignKeys::All), vec!["E0295"]);
}

#[test]
fn model_no_fk_under_none_is_redundant_w0110() {
    let src = format!("{ORG}@no_fk Order {{ id: Id  org: Org }}");
    assert_eq!(fk_codes(&src, ForeignKeys::None), vec!["W0110"]);
}

#[test]
fn actions_alone_never_trigger_a_reason_under_all() {
    // `on_delete`/`on_update` are concordant refinements under `all`; no reason, no lint.
    let src =
        format!("{ORG}Order {{ id: Id  org: Org @fk(on_delete: restrict, on_update: cascade) }}");
    assert!(fk_codes(&src, ForeignKeys::All).is_empty());
}
