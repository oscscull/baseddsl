//! Assemble a query's WHERE condition set: same-name/param conditions and `where` clauses,
//! then the injected soft-delete tombstone and `@scope` guards.

use super::super::*;
use super::optional::present_guard;

/// The full row filter for a query: its own `where`/params first, then the injected
/// soft-delete tombstone and `@scope` predicate. The one place their ordering lives, shared
/// by the row-query and aggregate-query lowering.
pub(crate) fn build_wheres(
    sel: &mut Select,
    q: &Query,
    root: &RModel,
    dialect: Dialect,
) -> Vec<String> {
    let mut wheres: Vec<String> = Vec::new();
    collect_filter(sel, q, root, &mut wheres);
    if let Some(sd) = &root.soft_delete {
        wheres.push(soft_pred(dialect, &sel.root_alias, root, sd));
    }
    // `@scope` rides into every query on the model unless the query opts out with
    // `unscoped(...)`. The injected predicate is the *chosen alternative* — the axes this
    // query named — resolved by sema per callable.
    if let Some(scope) = sel.scope_where(&sel.root_alias, root) {
        wheres.push(scope);
    }
    wheres
}

/// Append the query's filter conditions. Bare/inline queries map each param to a
/// same-name equality (or its per-param binding); block/inline queries also carry
/// explicit `where` clauses referencing params via `$`.
fn collect_filter(sel: &mut Select, q: &Query, root: &RModel, out: &mut Vec<String>) {
    sel.optional_params = q
        .params
        .iter()
        .filter(|p| p.optional)
        .map(|p| p.name.node.clone())
        .collect();
    let is_block = matches!(q.body, QueryBody::Block(_) | QueryBody::Raw(_));
    if !is_block {
        for p in &q.params {
            out.push(param_condition(sel, p, root));
        }
    }
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    for c in clauses {
        if let Clause::Where(pred) = c {
            out.push(sel.predicate(pred, root));
        }
    }
}

/// One bare/inline param -> a filter condition (per-param bindings). A `?` optional param's
/// predicate is wrapped in a present-guard so it drops when the arg is absent (queries.md) —
/// works for any operator, not just equality.
fn param_condition(sel: &mut Select, p: &Param, root: &RModel) -> String {
    let ph = format!(":{}", p.name.node);
    let raw = match &p.binding {
        // `user -> author`: equality on the named relation's FK column.
        Some(ParamBinding::Edge(edge)) => {
            let (alias, col) = sel.resolve(&single(&edge.node), root);
            format!("{} = {ph}", sel.qcol(&alias, &col))
        }
        // `since: timestamp > created_at`: explicit column + operator. The collection
        // ops mirror the predicate lowering — `in` takes a value list, `has` is JSON
        // containment (Postgres `col @> value`, MySQL-family `value MEMBER OF(col)`).
        Some(ParamBinding::ColOp { op, col }) => {
            let (alias, c) = sel.resolve(&single(&col.node), root);
            let lhs = sel.qcol(&alias, &c);
            match op {
                Op::In => format!("{lhs} IN ({ph})"),
                Op::Has => match sel.dialect {
                    Dialect::Postgres => format!("{lhs} @> {ph}"),
                    _ => format!("{ph} MEMBER OF({lhs})"),
                },
                _ => format!("{lhs} {} {ph}", sql_op(*op)),
            }
        }
        // same-name equality on the mapped column (a relation field maps to its FK).
        None => {
            let (alias, col) = sel.resolve(&single(&p.name.node), root);
            format!("{} = {ph}", sel.qcol(&alias, &col))
        }
    };
    present_guard(p, &raw)
}
