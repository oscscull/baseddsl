use super::*;

/// A query clause's field paths, rooted at `root`: a `where` predicate's columns and
/// an `order` clause's sort paths (a `page` clause carries none).
pub(crate) fn clause_paths<'a>(
    c: &'a Clause,
    root: &'a str,
    out: &mut Vec<(&'a str, &'a [Ident])>,
) {
    match c {
        Clause::Where(p) => pred_paths(p, root, out),
        Clause::Order(terms) => {
            for t in terms {
                out.push((root, &t.path.segments));
            }
        }
        // `group by` columns are model columns rooted at the query target (navigable /
        // renamable). A `having` predicate names shape-local aggregate aliases, not model
        // fields, so it contributes no field references.
        Clause::GroupBy(cols) => {
            for p in cols {
                out.push((root, &p.segments));
            }
        }
        Clause::Page(_) | Clause::Unindexed(_) | Clause::Having(_) => {}
    }
}

/// A predicate's field paths (both sides of a comparison, bare bool columns, filter-
/// call value paths), all rooted at `root`. Filter *names* are not fields.
pub(crate) fn pred_paths<'a>(
    p: &'a Predicate,
    root: &'a str,
    out: &mut Vec<(&'a str, &'a [Ident])>,
) {
    match p {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            pred_paths(a, root, out);
            pred_paths(b, root, out);
        }
        Predicate::Not(inner) => pred_paths(inner, root, out),
        Predicate::Cmp { path, value, .. } => {
            out.push((root, &path.segments));
            value_paths(value, root, out);
        }
        Predicate::InList { path, values } => {
            out.push((root, &path.segments));
            for v in values {
                value_paths(v, root, out);
            }
        }
        Predicate::Bare(path) => out.push((root, &path.segments)),
        Predicate::FilterCall { args, .. } => {
            for v in args {
                value_paths(v, root, out);
            }
        }
        Predicate::Raw(_) => {}
    }
}

/// A value's field path, when it is one (a column reference or a function argument
/// that is itself a column); params, literals, and `$name.field` step refs carry none.
pub(crate) fn value_paths<'a>(v: &'a Value, root: &'a str, out: &mut Vec<(&'a str, &'a [Ident])>) {
    match v {
        Value::Path(p) => out.push((root, &p.segments)),
        Value::Func(fc) => {
            for a in &fc.args {
                value_paths(a, root, out);
            }
        }
        _ => {}
    }
}

/// A mutation write body's field paths, rooted at each statement's write model,
/// recursing through `tx`. Covers `where` predicates and assign columns/values.
pub(crate) fn write_paths<'a>(body: &'a [WriteStmt], out: &mut Vec<(&'a str, &'a [Ident])>) {
    for w in body {
        match w {
            WriteStmt::Create {
                model,
                assigns,
                from: _,
                conflict,
                binding: _,
            } => {
                assign_paths(assigns, model.node.as_str(), out);
                if let Some(oc) = conflict {
                    assign_paths(&oc.update, model.node.as_str(), out);
                }
            }
            WriteStmt::Update {
                model,
                where_,
                assigns,
            } => {
                pred_paths(where_, model.node.as_str(), out);
                assign_paths(assigns, model.node.as_str(), out);
            }
            WriteStmt::Restore { model, where_ } => {
                pred_paths(where_, model.node.as_str(), out);
            }
            WriteStmt::Delete { model, where_ } | WriteStmt::HardDelete { model, where_ } => {
                if let Some(p) = where_ {
                    pred_paths(p, model.node.as_str(), out);
                }
            }
            WriteStmt::Tx(inner) => write_paths(inner, out),
            WriteStmt::Raw(_) => {}
        }
    }
}

/// A create/update's assign paths: the target column and any column-valued RHS.
pub(crate) fn assign_paths<'a>(
    assigns: &'a [Assign],
    model: &'a str,
    out: &mut Vec<(&'a str, &'a [Ident])>,
) {
    for a in assigns {
        out.push((model, std::slice::from_ref(&a.col)));
        assign_rhs_paths(&a.value, model, out);
    }
}

/// Field paths in an assignment RHS: a plain value's path, or every column operand
/// of an arithmetic expression (each rooted at the write model).
pub(crate) fn assign_rhs_paths<'a>(
    rhs: &'a AssignRhs,
    root: &'a str,
    out: &mut Vec<(&'a str, &'a [Ident])>,
) {
    match rhs {
        AssignRhs::Value(v) => value_paths(v, root, out),
        AssignRhs::Arith { lhs, rhs, .. } => {
            assign_rhs_paths(lhs, root, out);
            assign_rhs_paths(rhs, root, out);
        }
    }
}

// ---- Param / `$ctx` reference collectors ------------------------------------
