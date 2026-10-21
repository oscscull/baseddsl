use super::*;

/// `limit (N | $p[, max N])` rules: one per `list`, never beside `page`, and a param
/// limit names a required `int` param of a block body.
pub(super) fn check_limit(q: &Query, verb: Verb, sink: &mut Sink) {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => return,
    };
    let limits: Vec<&LimitClause> = clauses
        .iter()
        .filter_map(|c| match c {
            Clause::Limit(l) => Some(l),
            _ => None,
        })
        .collect();
    let Some(first) = limits.first() else {
        return;
    };
    for dup in &limits[1..] {
        placement(
            sink,
            dup.span,
            format!("query `{}` declares `limit` twice", q.name.node),
            "keep one `limit`",
        );
    }
    if verb == Verb::Get {
        placement(
            sink,
            first.span,
            format!("`get` query `{}` declares `limit`", q.name.node),
            "a `get` reads one row — `limit` caps a `list`",
        );
    }
    if clauses.iter().any(|c| matches!(c, Clause::Page(_))) {
        placement(
            sink,
            first.span,
            format!("query `{}` combines `limit` with `page`", q.name.node),
            "`page` already caps each page — drop one",
        );
    }
    check_limit_value(q, first, sink);
}

fn check_limit_value(q: &Query, l: &LimitClause, sink: &mut Sink) {
    if l.max == Some(0) {
        value(
            sink,
            l.span,
            "`limit` max must be at least 1".into(),
            "a max of 0 rejects every call",
        );
    }
    match &l.value {
        LimitValue::Lit(n) => {
            if *n == 0 {
                value(
                    sink,
                    l.span,
                    "`limit (0)` returns no rows".into(),
                    "use a limit of at least 1",
                );
            }
            if l.max.is_some() {
                value(
                    sink,
                    l.span,
                    "`max` on a literal `limit` has no effect".into(),
                    "`max` bounds a param limit — drop it, or take the limit from a param",
                );
            }
        }
        LimitValue::Param(r) if !r.path.is_empty() || r.optional => value(
            sink,
            l.span,
            "`limit` takes a plain `$param`".into(),
            "name a declared `int` param — `$ctx` fields and optional reads can't cap rows",
        ),
        LimitValue::Param(r) => check_limit_param(q, &r.name, sink),
    }
}

fn check_limit_param(q: &Query, name: &Ident, sink: &mut Sink) {
    let Some(p) = q.params.iter().find(|p| p.name.node == name.node) else {
        sink.error(
            code::UNKNOWN_PARAM,
            name.span,
            format!("`limit` names unknown param `${}`", name.node),
        );
        return;
    };
    if matches!(q.body, QueryBody::Inline(_)) {
        placement(
            sink,
            name.span,
            format!("`limit (${})` in an inline query body", name.node),
            "an inline body filters on every param — use a block body `{ list … limit ($p); }`",
        );
    }
    let is_int = p.ty.as_ref().is_some_and(|t| {
        matches!(t.base, BaseType::Primitive(Primitive::Int)) && !t.optional && !t.many
    });
    if !is_int || p.optional || p.binding.is_some() {
        value(
            sink,
            p.name.span,
            format!("`limit` param `{}` must be a required `int`", p.name.node),
            "declare it as `limit: int`",
        );
    }
    if let Some(DefaultVal::Lit(Literal::Int(d))) = &p.default {
        if *d < 1 {
            value(
                sink,
                p.name.span,
                format!("`limit` param `{}` defaults to {d}", p.name.node),
                "a limit must be at least 1",
            );
        }
    }
}

fn placement(sink: &mut Sink, span: Span, msg: String, note: &str) {
    sink.error_note(code::LIMIT_PLACEMENT, span, msg, note);
}

fn value(sink: &mut Sink, span: Span, msg: String, note: &str) {
    sink.error_note(code::LIMIT_VALUE, span, msg, note);
}

/// Whether `name` is the param a `limit ($name)` reads.
pub(super) fn is_limit_param(q: &Query, name: &str) -> bool {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => return false,
    };
    clauses.iter().any(|c| {
        matches!(c, Clause::Limit(LimitClause { value: LimitValue::Param(p), .. }) if p.name.node == name)
    })
}
