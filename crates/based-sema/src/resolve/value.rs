use super::*;

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

pub(crate) fn check_raw_params(raw: &RawSql, params: &[String], sink: &mut Sink) {
    for part in &raw.parts {
        if let RawPart::Param(pr) = part {
            check_param_ref(pr, params, sink);
        }
    }
}

pub fn check_sort_term(t: &SortTerm, model: usize, cx: &Cx, sink: &mut Sink) {
    if let Some(term) = resolve_path(&t.path, model, cx, sink) {
        reject_opaque(&term, &t.path, "sort", sink);
    }
}

/// An opaque `raw(…)` column has no type the engine can compare or order by, so it is
/// never a filter / sort / group / aggregate operand. The read side stays open
/// through the `raw` leaf — a raw predicate term or a raw shape value (`ST_Area(geom)`),
/// where the SQL author owns the semantics.
pub fn reject_opaque(term: &Terminal, path: &Path, what: &str, sink: &mut Sink) -> bool {
    let Terminal::Opaque(ty) = term else {
        return false;
    };
    let Some(seg) = path.segments.last() else {
        return false;
    };
    sink.error_note(
        code::OPAQUE_OPERAND,
        seg.span,
        format!("cannot {what} by `{}` — a {ty} column is opaque", seg.node),
        "the engine does not model this type; reach it with a `raw` predicate term or a raw shape value",
    );
    true
}
