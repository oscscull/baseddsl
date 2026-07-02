//! Golden conformance harness for sema. Each `tests/conformance-sema/<case>/`
//! holds an `input.bsl` and an `expected` summary of *resolution + diagnostics*
//! (the parser has its own goldens under `tests/conformance/`). Re-bless with
//! `BLESS=1 cargo test -p based-sema --test conformance`.
//!
//! The summary is deliberately coarse — the resolution facts that are *not* in
//! the AST (table names, relation kinds, inferred verb/target, soft-delete mode,
//! inferred indexes, `$ctx` requirements) plus the diagnostics, rendered stably
//! so goldens stay legible in review and don't churn on unrelated field additions.

use based_ast::{DefaultVal, FileId, Primitive, SortDir, SortTerm, Verb};
use based_parser::parse_file;
use based_sema::{check, CheckedSchema, CtxField, MemberKind, RModel, SoftDelete, SoftMode};
use std::path::{Path, PathBuf};

fn conformance_dir() -> PathBuf {
    // crates/based-sema -> repo root -> tests/conformance-sema
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance-sema")
        .canonicalize()
        .expect("tests/conformance-sema dir")
}

#[test]
fn conformance_golden() {
    let dir = conformance_dir();
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read conformance-sema dir")
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
        let got = summarize(&input);
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

/// Parse + check + render a stable summary of both the diagnostics and the
/// resolved schema. A parse failure short-circuits to `PARSE-ERR` (sema goldens
/// assume parseable input; malformed input belongs in the parser goldens).
fn summarize(src: &str) -> String {
    let sf = match parse_file(src, FileId(0)) {
        Ok(sf) => sf,
        Err(diags) => {
            let mut out = String::from("PARSE-ERR\n");
            for d in &diags {
                out.push_str(&format!("  {}: {}\n", d.code, d.message));
            }
            return out;
        }
    };
    let (schema, mut diags) = check(&sf.decls);

    let mut out = String::new();

    // Diagnostics first, sorted by (code, message) so the golden is order-
    // independent — sema's emission order is a pass-ordering detail, not a fact
    // worth pinning.
    out.push_str("DIAGNOSTICS\n");
    diags.sort_by(|a, b| a.code.cmp(b.code).then(a.message.cmp(&b.message)));
    if diags.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for d in &diags {
            out.push_str(&format!("  {}: {}\n", d.code, d.message));
        }
    }

    out.push_str("SCHEMA\n");
    out.push_str(&summarize_schema(&schema));
    out
}

fn summarize_schema(s: &CheckedSchema) -> String {
    let mut out = String::new();
    for m in &s.models {
        out.push_str(&summarize_model(m));
    }
    for sh in &s.shapes {
        out.push_str(&format!("shape {} from {}\n", sh.name, sh.from));
    }
    for q in &s.queries {
        out.push_str(&format!(
            "query {}  target={} verb={}{}{}{}{}\n",
            q.name,
            q.target,
            verb(q.verb),
            if q.many { " many" } else { "" },
            match &q.ret_shape {
                Some(sh) => format!("  shape={sh}"),
                None => String::new(),
            },
            if q.paginated { "  page" } else { "" },
            ctx(&q.ctx_requires),
        ));
    }
    for mu in &s.mutations {
        out.push_str(&format!(
            "mutation {}  ret={}{}\n",
            mu.name,
            mu.ret_model,
            ctx(&mu.ctx_requires),
        ));
    }
    for f in &s.filters {
        out.push_str(&format!("filter {}  arity={}\n", f.name, f.arity));
    }
    out
}

fn summarize_model(m: &RModel) -> String {
    let mut head = format!("model {}  table={}", m.name, m.table);
    if let Some(SoftDelete { field, mode }) = &m.soft_delete {
        head.push_str(&format!(
            "  soft_delete={field}({})",
            match mode {
                SoftMode::Timestamp => "timestamp",
                SoftMode::Bool => "bool",
            }
        ));
    }
    if m.scope.is_some() {
        head.push_str("  scope");
    }
    if let Some(t) = &m.tenant {
        head.push_str(&format!("  tenant={t}"));
    }
    if !m.sort.is_empty() {
        head.push_str(&format!("  sort=[{}]", sorts(&m.sort)));
    }
    let mut out = format!("{head}\n");

    for mem in &m.members {
        out.push_str(&format!("  {}\n", member(&mem.name, &mem.kind)));
    }
    for ix in &m.indexes {
        out.push_str(&format!(
            "  index{}({})\n",
            if ix.unique { " unique" } else { "" },
            ix.columns.join(", ")
        ));
    }
    for ix in &m.inferred_indexes {
        out.push_str(&format!("  index inferred({})\n", ix.columns.join(", ")));
    }
    out
}

fn member(name: &str, kind: &MemberKind) -> String {
    match kind {
        MemberKind::Scalar {
            ty,
            optional,
            many,
            column,
            unique,
            default,
        } => {
            let mut s = format!(
                "{name}: {}{}{}",
                prim(*ty),
                if *optional { "?" } else { "" },
                if *many { "[]" } else { "" },
            );
            if column != name {
                s.push_str(&format!("  col={column}"));
            }
            if *unique {
                s.push_str("  unique");
            }
            if let Some(d) = default {
                s.push_str(&format!("  default={}", default_val(d)));
            }
            s
        }
        MemberKind::Forward {
            target,
            optional,
            fk_col,
            custom_join,
        } => format!(
            "{name}: -> {target}{}  fk={fk_col}{}",
            if *optional { "?" } else { "" },
            if *custom_join { "  custom-join" } else { "" },
        ),
        MemberKind::Inverse { target, via } => format!("{name}: <- {target} via {via}"),
    }
}

fn prim(p: Primitive) -> &'static str {
    match p {
        Primitive::Text => "text",
        Primitive::Int => "int",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "timestamp",
        Primitive::Date => "date",
        Primitive::Json => "json",
        Primitive::Uuid => "uuid",
        Primitive::Id => "Id",
    }
}

fn verb(v: Verb) -> &'static str {
    match v {
        Verb::Get => "get",
        Verb::List => "list",
    }
}

fn sorts(terms: &[SortTerm]) -> String {
    terms
        .iter()
        .map(|t| {
            let path = t
                .path
                .segments
                .iter()
                .map(|s| s.node.as_str())
                .collect::<Vec<_>>()
                .join(".");
            let dir = match t.dir {
                SortDir::Asc => "asc",
                SortDir::Desc => "desc",
            };
            format!("{path} {dir}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn ctx(reqs: &[based_sema::CtxReq]) -> String {
    if reqs.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = reqs
        .iter()
        .map(|r| {
            let ty = match &r.ty {
                CtxField::Scalar(p) => prim(*p).to_string(),
                CtxField::Relation(m) => format!("-> {m}"),
            };
            format!("{}: {ty}", r.field)
        })
        .collect();
    parts.sort();
    format!("  ctx=[{}]", parts.join(", "))
}

fn default_val(d: &DefaultVal) -> String {
    match d {
        DefaultVal::Lit(_) => "lit".to_string(),
        DefaultVal::Func(f) => format!("{}()", f.name.node),
    }
}
