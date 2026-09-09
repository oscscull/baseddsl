//! Render a relation's custom `on:` condition to join-`ON` SQL.

use super::*;

/// Render a relation's custom `on:` join condition (`on: order.user_ref =
/// user.legacy_id`) as the join `ON` SQL. Unlike a `where` predicate — resolved
/// against one model's field paths — a join condition names both joined models
/// explicitly by table (`<table>.<column>`): the FK-holding `near` model and its
/// far target. A column qualified by `near_table` resolves to `near_alias`; any
/// other qualifier is the far side, resolving to `far_alias`. Sema
/// (`resolve::check_relation_on`) has already verified every path is
/// `<table>.<column>` naming one of the two models with a real column, and that no
/// `$param` / filter / function appears — so the value arm need only handle a
/// column path or a literal.
///
/// A self-referential custom join (near and far are the same table) can't be
/// disambiguated by table name, so sema rejects it up front (`E0127`); this
/// renderer therefore only ever sees a two-model join.
pub(crate) fn render_join_on(
    dialect: Dialect,
    pred: &Predicate,
    near_table: &str,
    near_alias: &str,
    far_alias: &str,
) -> String {
    let col = |p: &Path| -> String {
        let table = p.segments.first().map_or("", |s| s.node.as_str());
        let column = p.segments.get(1).map_or("", |s| s.node.as_str());
        let alias = if table == near_table {
            near_alias
        } else {
            far_alias
        };
        dialect.qcol(alias, column)
    };
    let val = |v: &Value| -> String {
        match v {
            Value::Path(p) => col(p),
            Value::Lit(l) => render_lit(dialect, l),
            // Sema rejects params/functions in a join condition; guard defensively.
            _ => "NULL".to_string(),
        }
    };
    let recur = |p| render_join_on(dialect, p, near_table, near_alias, far_alias);
    match pred {
        Predicate::And(a, b) => format!("({} AND {})", recur(a), recur(b)),
        Predicate::Or(a, b) => format!("({} OR {})", recur(a), recur(b)),
        Predicate::Not(inner) => format!("NOT ({})", recur(inner)),
        Predicate::Cmp { path, op, value } => {
            let (lhs, rhs) = (col(path), val(value));
            match op {
                Op::In => format!("{lhs} IN ({rhs})"),
                Op::Has => match dialect {
                    Dialect::Postgres => format!("{lhs} @> {rhs}"),
                    _ => format!("{rhs} MEMBER OF({lhs})"),
                },
                _ => format!("{lhs} {} {rhs}", sql_op(*op)),
            }
        }
        Predicate::InList { path, values } => {
            let items: Vec<String> = values.iter().map(&val).collect();
            format!("{} IN ({})", col(path), items.join(", "))
        }
        Predicate::Bare(path) => format!("{} = {}", col(path), dialect.bool_lit(true)),
        Predicate::Raw(raw) => format!("({})", render_raw(dialect, raw, near_alias, near_table)),
        // Sema rejects a named-filter call in a join; lower to a harmless truth.
        Predicate::FilterCall { .. } => "TRUE".to_string(),
    }
}
