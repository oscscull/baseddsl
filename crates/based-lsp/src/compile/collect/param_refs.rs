use super::*;

/// A callable's declared params, or an empty slice for a non-callable declaration.
pub(crate) fn decl_params(d: &Decl) -> &[Param] {
    match d {
        Decl::Query(q) => &q.params,
        Decl::Mutation(m) => &m.params,
        Decl::Filter(f) => &f.params,
        _ => &[],
    }
}

/// Every `$param` / `$ctx.field` reference in a callable's body (query clauses,
/// mutation writes, or a filter predicate) — the sites that name a param or bag field.
pub(crate) fn callable_param_refs(d: &Decl) -> Vec<&ParamRef> {
    let mut out = Vec::new();
    match d {
        Decl::Query(q) => match &q.body {
            QueryBody::Inline(cs) => cs.iter().for_each(|c| clause_param_refs(c, &mut out)),
            QueryBody::Block(stmt) => stmt
                .clauses
                .iter()
                .for_each(|c| clause_param_refs(c, &mut out)),
            QueryBody::Raw(r) => raw_param_refs(r, &mut out),
            QueryBody::Bare => {}
        },
        Decl::Mutation(m) => write_param_refs(&m.body, &mut out),
        Decl::Filter(f) => pred_param_refs(&f.pred, &mut out),
        _ => {}
    }
    out
}

pub(crate) fn clause_param_refs<'a>(c: &'a Clause, out: &mut Vec<&'a ParamRef>) {
    if let Clause::Where(p) = c {
        pred_param_refs(p, out);
    }
}

/// Every `create … as name` step-binding declaration ident in a callable body — a
/// mutation only (queries/filters have no writes), recursing through `tx`.
pub(crate) fn callable_binding_decls(d: &Decl) -> Vec<&Ident> {
    let mut out = Vec::new();
    if let Decl::Mutation(m) = d {
        collect_binding_decls(&m.body, &mut out);
    }
    out
}

pub(crate) fn collect_binding_decls<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a Ident>) {
    for w in body {
        match w {
            WriteStmt::Create {
                binding: Some(b), ..
            } => out.push(b),
            WriteStmt::Tx(inner) => collect_binding_decls(inner, out),
            _ => {}
        }
    }
}

/// The model name of the `create … as name` binding whose decl span is `target`, if it
/// is in this write body (recursing through `tx`).
pub(crate) fn binding_model(body: &[WriteStmt], target: Span) -> Option<&str> {
    for w in body {
        match w {
            WriteStmt::Create {
                model,
                binding: Some(b),
                ..
            } if b.span == target => return Some(model.node.as_str()),
            WriteStmt::Tx(inner) => {
                if let Some(m) = binding_model(inner, target) {
                    return Some(m);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn write_param_refs<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a ParamRef>) {
    for w in body {
        match w {
            WriteStmt::Create {
                assigns, conflict, ..
            } => {
                for a in assigns {
                    assign_rhs_param_refs(&a.value, out);
                }
                if let Some(oc) = conflict {
                    for a in &oc.update {
                        assign_rhs_param_refs(&a.value, out);
                    }
                }
            }
            WriteStmt::Update {
                where_, assigns, ..
            } => {
                pred_param_refs(where_, out);
                for a in assigns {
                    assign_rhs_param_refs(&a.value, out);
                }
            }
            WriteStmt::Restore { where_, .. } => pred_param_refs(where_, out),
            WriteStmt::Delete { where_, .. } | WriteStmt::HardDelete { where_, .. } => {
                if let Some(p) = where_ {
                    pred_param_refs(p, out);
                }
            }
            WriteStmt::Tx(inner) => write_param_refs(inner, out),
            WriteStmt::Raw(r) => raw_param_refs(r, out),
        }
    }
}

pub(crate) fn pred_param_refs<'a>(p: &'a Predicate, out: &mut Vec<&'a ParamRef>) {
    match p {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            pred_param_refs(a, out);
            pred_param_refs(b, out);
        }
        Predicate::Not(inner) => pred_param_refs(inner, out),
        Predicate::Cmp { value, .. } => value_param_refs(value, out),
        Predicate::InList { values, .. } => values.iter().for_each(|v| value_param_refs(v, out)),
        Predicate::FilterCall { args, .. } => args.iter().for_each(|v| value_param_refs(v, out)),
        Predicate::Raw(r) => raw_param_refs(r, out),
        Predicate::Bare(_) => {}
    }
}

/// Every `$param` reference in an assignment RHS (a plain value, or each operand of
/// an arithmetic expression).
pub(crate) fn assign_rhs_param_refs<'a>(rhs: &'a AssignRhs, out: &mut Vec<&'a ParamRef>) {
    match rhs {
        AssignRhs::Value(v) => value_param_refs(v, out),
        AssignRhs::Arith { lhs, rhs, .. } => {
            assign_rhs_param_refs(lhs, out);
            assign_rhs_param_refs(rhs, out);
        }
    }
}

pub(crate) fn value_param_refs<'a>(v: &'a Value, out: &mut Vec<&'a ParamRef>) {
    match v {
        Value::Param(pr) => out.push(pr),
        Value::Func(fc) => fc.args.iter().for_each(|a| value_param_refs(a, out)),
        _ => {}
    }
}

pub(crate) fn raw_param_refs<'a>(r: &'a RawSql, out: &mut Vec<&'a ParamRef>) {
    for part in &r.parts {
        if let RawPart::Param(pr) = part {
            out.push(pr);
        }
    }
}
