//! Golden conformance harness. Each `tests/conformance/<case>/` holds an
//! `input.bsl` and an `expected` summary of the parse. Re-bless with
//! `BLESS=1 cargo test -p based-parser --test conformance`.

use based_ast::*;
use based_diagnostics::Diagnostic;
use based_parser::parse_file;
use std::path::{Path, PathBuf};

fn conformance_dir() -> PathBuf {
    // crates/based-parser -> repo root -> tests/conformance
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance")
        .canonicalize()
        .expect("tests/conformance dir")
}

#[test]
fn conformance_golden() {
    let dir = conformance_dir();
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read conformance dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no conformance cases found in {dir:?}");

    let bless = std::env::var_os("BLESS").is_some();
    let mut failures = Vec::new();

    for case in cases {
        let input = std::fs::read_to_string(case.join("input.bsl"))
            .unwrap_or_else(|e| panic!("read {}: {e}", case.join("input.bsl").display()));
        let got = summarize(parse_file(&input, FileId(0)));
        let expected_path = case.join("expected");

        if bless {
            std::fs::write(&expected_path, &got).expect("write expected");
            continue;
        }

        let want = std::fs::read_to_string(&expected_path).unwrap_or_default();
        if got.trim_end() != want.trim_end() {
            failures.push(format!(
                "--- case {} ---\nEXPECTED:\n{}\nGOT:\n{}",
                case.file_name().unwrap().to_string_lossy(),
                want.trim_end(),
                got.trim_end()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "conformance mismatch (re-bless with BLESS=1):\n\n{}",
        failures.join("\n\n")
    );
}

/// Render a stable, human-readable summary of a parse result. Deliberately
/// coarse (counts and shapes, not full AST dumps) so goldens stay legible in
/// review and don't churn on unrelated field additions.
fn summarize(res: Result<SchemaFile, Vec<Diagnostic>>) -> String {
    let mut out = String::new();
    match res {
        Ok(sf) => {
            out.push_str("OK\n");
            for decl in &sf.decls {
                out.push_str(&summarize_decl(decl));
                out.push('\n');
            }
        }
        Err(diags) => {
            out.push_str("ERR\n");
            for d in &diags {
                out.push_str(&format!("{}: {}\n", d.code, d.message));
            }
        }
    }
    out
}

fn summarize_decl(decl: &Decl) -> String {
    match decl {
        Decl::Model(m) => {
            let decos: Vec<&str> = m.decorators.iter().map(|d| d.name.node.as_str()).collect();
            let fields = m
                .members
                .iter()
                .filter(|x| matches!(x, Member::Field(_)))
                .count();
            let indexes = m
                .members
                .iter()
                .filter(|x| matches!(x, Member::Index(_)))
                .count();
            format!(
                "model {}  decorators=[{}]  fields={fields}  indexes={indexes}",
                m.name.node,
                decos.join(", ")
            )
        }
        Decl::Shape(s) => format!(
            "shape {} from {}  fields={}",
            s.name.node,
            s.from.node,
            s.body.len()
        ),
        Decl::Query(q) => {
            let body = match q.body {
                QueryBody::Bare => "bare",
                QueryBody::Inline(_) => "inline",
                QueryBody::Block(_) => "block",
            };
            format!(
                "query {}  params={}  ret={}{}  body={body}",
                q.name.node,
                q.params.len(),
                q.ret.ty.node,
                if q.ret.many { "[]" } else { "" }
            )
        }
        Decl::Mutation(m) => format!(
            "mutation {}  params={}  ret={}{}  writes={}",
            m.name.node,
            m.params.len(),
            m.ret.ty.node,
            if m.ret.many { "[]" } else { "" },
            m.body.len()
        ),
        Decl::Filter(f) => format!("filter {}  params={}", f.name.node, f.params.len()),
        Decl::Scope(s) => format!("scope {}  terms={}", s.name.node, s.terms.len()),
        Decl::Enum(e) => {
            let vs = e
                .variants
                .iter()
                .map(|v| match v.value.as_ref().map(|s| &s.node) {
                    None => v.name.node.clone(),
                    Some(based_ast::VariantValue::Str(s)) => format!("{}=\"{}\"", v.name.node, s),
                    Some(based_ast::VariantValue::Int(n)) => format!("{}={}", v.name.node, n),
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("enum {}  variants=[{vs}]", e.name.node)
        }
    }
}
