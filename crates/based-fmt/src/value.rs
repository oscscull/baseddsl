//! The leaf value forms: a value (param/path/literal/func), a param reference, a
//! dotted path, a function call, a default value, a literal, a sort term, and the
//! string-body escaper the printer re-quotes around.

use based_ast::*;

pub(crate) fn value(v: &Value) -> String {
    match v {
        Value::Param(pr) => param_ref(pr),
        Value::Path(p) => path(p),
        Value::Lit(l) => literal(l),
        Value::Func(f) => func_call(f),
    }
}

pub(crate) fn param_ref(pr: &ParamRef) -> String {
    let mut s = format!("${}", pr.name.node);
    for seg in &pr.path {
        s.push('.');
        s.push_str(&seg.node);
    }
    // The trailing `?` marks an optional context read.
    if pr.optional {
        s.push('?');
    }
    s
}

pub(crate) fn path(p: &Path) -> String {
    p.segments
        .iter()
        .map(|s| s.node.clone())
        .collect::<Vec<_>>()
        .join(".")
}

fn func_call(f: &FuncCall) -> String {
    format!(
        "{}({})",
        f.name.node,
        f.args.iter().map(value).collect::<Vec<_>>().join(", ")
    )
}

pub(crate) fn default_val(d: &DefaultVal) -> String {
    match d {
        DefaultVal::Lit(l) => literal(l),
        DefaultVal::Func(f) => func_call(f),
        DefaultVal::Variant(v) => v.node.clone(),
    }
}

pub(crate) fn literal(l: &Literal) -> String {
    match l {
        Literal::Str(s) => format!("\"{}\"", esc(s)),
        Literal::Int(n) => n.to_string(),
        // Emitted verbatim — the exact source text is preserved.
        Literal::Decimal(s) => s.clone(),
        Literal::Bool(b) => b.to_string(),
        Literal::Null => "null".to_string(),
    }
}

pub(crate) fn sort_term(s: &SortTerm) -> String {
    let mut out = match s.dir {
        SortDir::Desc => format!("{} desc", path(&s.path)),
        SortDir::Asc => path(&s.path),
    };
    match s.nulls {
        Some(NullsPlacement::First) => out.push_str(" nulls first"),
        Some(NullsPlacement::Last) => out.push_str(" nulls last"),
        None => {}
    }
    out
}

/// Escape a string literal's body (the printer re-adds the surrounding quotes).
pub(crate) fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
