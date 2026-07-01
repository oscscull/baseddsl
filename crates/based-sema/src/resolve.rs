//! Path resolution + the shared predicate/value checker.
//!
//! One expression language is used in `where`, `@scope`, named filters, and
//! relation joins (queries.md), so this module is the single place paths, params,
//! filter calls, and functions are validated. Path resolution walks declared
//! fields: forward *and* backward edges are just fields, so forward traversal
//! needs no inverse and backward traversal works exactly because the inverse was
//! declared (relations.md).

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;

/// Read-only resolution context shared by every checker pass.
pub struct Cx<'a> {
    pub models: &'a [RModel],
    /// model name -> index into `models`.
    pub index: &'a HashMap<String, usize>,
    /// named filter -> parameter count (for call arity checks).
    pub filters: &'a HashMap<String, usize>,
    /// shape name -> the model it projects (`from`). Used to resolve return types.
    pub shapes: &'a HashMap<String, String>,
}

impl<'a> Cx<'a> {
    pub fn model(&self, i: usize) -> &RModel {
        &self.models[i]
    }
    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }
}

/// What a dotted path lands on. The payload is the resolved type/target — unused
/// today (resolution only reports name errors), but carried for the operand
/// type-checking pass (PLAN.md).
#[allow(dead_code)]
pub enum Terminal {
    Scalar(Primitive),
    /// A relation edge; `.0` is the target model name. Comparable to its key (Id).
    Relation(String),
}

/// Resolve `path` from model `start`. Reports the first failure and returns `None`.
pub fn resolve_path(path: &Path, start: usize, cx: &Cx, sink: &mut Sink) -> Option<Terminal> {
    let mut cur = start;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let Some(mem) = cx.model(cur).member(&seg.node) else {
            sink.error(
                code::UNKNOWN_FIELD,
                seg.span,
                format!("`{}` has no field `{}`", cx.model(cur).name, seg.node),
            );
            return None;
        };
        let last = i + 1 == n;
        match &mem.kind {
            MemberKind::Scalar { ty, .. } => {
                if last {
                    return Some(Terminal::Scalar(*ty));
                }
                sink.error(
                    code::TRAVERSE_SCALAR,
                    seg.span,
                    format!("cannot traverse into scalar column `{}`", seg.node),
                );
                return None;
            }
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                if last {
                    return Some(Terminal::Relation(target.clone()));
                }
                // Target existence is validated during model checking; if it is
                // missing, that error already fired — just stop here.
                cur = cx.find(target)?;
            }
        }
    }
    None
}

/// Check one predicate against an optional model context. `params` is the set of
/// legal `$`-parameter names (besides `$ctx`, always allowed, D4). When `model`
/// is `None` (a named filter, resolved without a caller), column paths are not
/// bound to a model — only params, filter calls, and functions are checked.
pub fn check_predicate(
    pred: &Predicate,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    match pred {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            check_predicate(a, model, cx, params, sink);
            check_predicate(b, model, cx, params, sink);
        }
        Predicate::Not(p) => check_predicate(p, model, cx, params, sink),
        Predicate::Cmp { path, value, .. } => {
            if let Some(mi) = model {
                resolve_path(path, mi, cx, sink);
            }
            check_value(value, model, cx, params, sink);
        }
        Predicate::Bare(path) => {
            // A bare atom is a bool column or a zero-arg named-filter reference.
            if path.segments.len() == 1 && cx.filters.contains_key(&path.segments[0].node) {
                return;
            }
            if let Some(mi) = model {
                resolve_path(path, mi, cx, sink);
            }
        }
        Predicate::FilterCall { name, args } => {
            match cx.filters.get(&name.node) {
                None => sink.error(
                    code::UNKNOWN_FILTER,
                    name.span,
                    format!("unknown filter `{}`", name.node),
                ),
                Some(&arity) if arity != args.len() => sink.error(
                    code::FILTER_ARITY,
                    name.span,
                    format!(
                        "filter `{}` takes {arity} argument(s), got {}",
                        name.node,
                        args.len()
                    ),
                ),
                Some(_) => {}
            }
            for v in args {
                check_value(v, model, cx, params, sink);
            }
        }
        Predicate::Raw(raw) => {
            check_raw_params(raw, params, sink);
            if let Some(mi) = model {
                if let Some(sd) = &cx.model(mi).soft_delete {
                    sink.warn(
                        code::RAW_SOFT_DELETE_GAP,
                        raw.span,
                        format!(
                            "raw SQL on soft-delete model `{}`: engine can't verify the `{}` tombstone filter — confirm it",
                            cx.model(mi).name, sd.field
                        ),
                    );
                }
            }
        }
    }
}

pub fn check_value(
    value: &Value,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    match value {
        Value::Param(pr) => check_param_ref(pr, params, sink),
        Value::Path(path) => {
            if let Some(mi) = model {
                resolve_path(path, mi, cx, sink);
            }
        }
        Value::Lit(_) => {}
        Value::Func(f) => check_func(f, model, cx, params, sink),
    }
}

pub fn check_param_ref(pr: &ParamRef, params: &[String], sink: &mut Sink) {
    // `$ctx` is a reserved namespace typed by the manifest (D4) — always legal.
    if pr.name.node == "ctx" {
        return;
    }
    if !params.iter().any(|p| p == &pr.name.node) {
        sink.error(
            code::UNKNOWN_PARAM,
            pr.name.span,
            format!("unknown parameter `${}`", pr.name.node),
        );
    }
}

pub fn check_func(f: &FuncCall, model: Option<usize>, cx: &Cx, params: &[String], sink: &mut Sink) {
    if !KNOWN_FUNCS.contains(&f.name.node.as_str()) {
        sink.error(
            code::UNKNOWN_FUNC,
            f.name.span,
            format!(
                "unknown function `{}` (available: {})",
                f.name.node,
                KNOWN_FUNCS.join(", ")
            ),
        );
    }
    for a in &f.args {
        check_value(a, model, cx, params, sink);
    }
}

/// A field/param default: only its function (e.g. `now()`) needs checking.
pub fn check_default(dv: &DefaultVal, sink: &mut Sink) {
    if let DefaultVal::Func(f) = dv {
        if !KNOWN_FUNCS.contains(&f.name.node.as_str()) {
            sink.error(
                code::UNKNOWN_FUNC,
                f.name.span,
                format!(
                    "unknown function `{}` (available: {})",
                    f.name.node,
                    KNOWN_FUNCS.join(", ")
                ),
            );
        }
    }
}

fn check_raw_params(raw: &RawSql, params: &[String], sink: &mut Sink) {
    for part in &raw.parts {
        if let RawPart::Param(pr) = part {
            check_param_ref(pr, params, sink);
        }
    }
}

pub fn check_sort_term(t: &SortTerm, model: usize, cx: &Cx, sink: &mut Sink) {
    resolve_path(&t.path, model, cx, sink);
}
