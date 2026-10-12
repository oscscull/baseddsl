//! The automatic reverse of a literal-row `create`: delete the created rows by a unique key
//! every row spells out as a literal.

use based_ast::{DataOp, DataStep, Literal, ShapeExpr, Value};

use super::super::model::Snapshot;
use super::table::TableView;
use crate::sql::dml::render_lit;
use crate::Dialect;

/// `DELETE FROM t WHERE key IN (…)` undoing `step`, or `None` when it isn't a plain literal
/// `create` keyed by literals (an upsert, an update, or a delete can't be undone).
pub(crate) fn invert(step: &DataStep, snap: &Snapshot, dialect: Dialect) -> Option<String> {
    let (
        DataOp::Create {
            table,
            rows,
            conflict: None,
            ..
        },
        None,
    ) = (&step.op, &step.chunk)
    else {
        return None;
    };
    let t = TableView::find(snap, &table.node)?;
    t.unique_keys().into_iter().find_map(|key| {
        let tuples = rows
            .iter()
            .map(|row| {
                key.iter()
                    .map(|k| {
                        let a = row.assigns.iter().find(|a| &a.col.node == k)?;
                        literal(&a.value, dialect)
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .collect::<Option<Vec<_>>>()?;
        let qt = dialect.quote_table(t.snap.schema.as_deref(), t.name());
        let cols: Vec<String> = key.iter().map(|k| dialect.quote(k)).collect();
        let vals: Vec<String> = tuples.iter().map(|v| tuple(v)).collect();
        Some(format!(
            "DELETE FROM {qt} WHERE {} IN ({})",
            tuple(&cols),
            vals.join(", ")
        ))
    })
}

/// One item bare, several as a parenthesized SQL row value.
fn tuple(items: &[String]) -> String {
    match items {
        [one] => one.clone(),
        _ => format!("({})", items.join(", ")),
    }
}

/// A non-null literal (or a bare enum value) as SQL.
fn literal(e: &ShapeExpr, dialect: Dialect) -> Option<String> {
    match e {
        ShapeExpr::Value(Value::Lit(Literal::Null)) => None,
        ShapeExpr::Value(Value::Lit(l)) => Some(render_lit(dialect, l)),
        _ => None,
    }
}
