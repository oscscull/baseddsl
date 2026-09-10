use std::collections::HashMap;

use super::*;

/// The upsert tail (`ON CONFLICT (cols) DO UPDATE SET …` / `ON DUPLICATE KEY UPDATE …`) and
/// the conflict-target read-back key (each target column's set value), or `("", None)` for a
/// plain create. The conflict key reads the winning row back on both the insert and conflict
/// paths.
pub(crate) fn upsert_tail_and_key(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: &RModel,
    conflict: Option<&OnConflict>,
    dialect: Dialect,
    value_by_field: &std::collections::HashMap<String, String>,
    incoming: bool,
) -> (String, Option<Vec<(String, String)>>) {
    let Some(oc) = conflict else {
        return (String::new(), None);
    };
    // The inline-create upsert has no proposed-row spread (`...incoming` is a bulk-only
    // form), so the spread column set is always empty on this path.
    let sets = conflict_update_sets(schema, decls, model, oc, dialect, incoming, &[]);
    let key: Vec<(String, String)> = oc
        .target
        .iter()
        .filter_map(|t| {
            value_by_field
                .get(&t.node)
                .map(|v| (physical_col(model, &t.node), v.clone()))
        })
        .collect();
    (upsert_tail(dialect, oc, model, &sets), Some(key))
}

/// The `SET col = value` fragments for an upsert's `update` branch. Columns render **bare**
/// on both sides (a bare RHS column names the existing row on every dialect; a qualified
/// one is rejected/ambiguous in the conflict clause), reusing the ordinary assign lowering
/// (enum variants → wire literals, `hits = hits + 1` → the atomic arithmetic).
pub(crate) fn conflict_update_sets(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: &RModel,
    oc: &OnConflict,
    dialect: Dialect,
    incoming: bool,
    spread_cols: &[String],
) -> Vec<String> {
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_bare_cols(true)
        .with_incoming(incoming);
    let mut render = |a: &Assign| -> (String, String) {
        (
            physical_col(model, &a.col.node),
            sel.assign_rhs(&a.value, model, &a.col.node),
        )
    };
    // Build the (col, rhs) list in lexical order: a `...incoming` spread expands, at its own
    // position, to every eligible payload column set from the proposed row.
    let mut items: Vec<(String, String)> = Vec::new();
    match oc.spread.as_ref().map(|s| s.preceding.min(oc.update.len())) {
        Some(preceding) => {
            items.extend(oc.update[..preceding].iter().map(&mut render));
            items.extend(
                spread_cols
                    .iter()
                    .map(|c| (c.clone(), incoming_col_ref(dialect, c))),
            );
            items.extend(oc.update[preceding..].iter().map(&mut render));
        }
        None => items.extend(oc.update.iter().map(&mut render)),
    }
    // Last-write-wins (JS object-spread): a column set twice keeps its last RHS. SQL's SET
    // list rejects a duplicate column, so collapse to one assignment per column.
    let mut order: Vec<String> = Vec::new();
    let mut vals: HashMap<String, String> = HashMap::new();
    for (col, val) in items {
        if !vals.contains_key(&col) {
            order.push(col.clone());
        }
        vals.insert(col, val);
    }
    order
        .into_iter()
        .map(|col| format!("{} = {}", dialect.quote(&col), vals[&col]))
        .collect()
}

/// A bare physical column read from the proposed/incoming row of a bulk upsert — the spread
/// counterpart of an `incoming.<col>` operand.
fn incoming_col_ref(dialect: Dialect, col: &str) -> String {
    match dialect {
        Dialect::Postgres | Dialect::Sqlite => format!("excluded.{}", dialect.quote(col)),
        Dialect::MariaDb | Dialect::MySql => format!("VALUES({})", dialect.quote(col)),
    }
}

/// The per-dialect upsert clause appended to the INSERT: Postgres/SQLite carry the explicit
/// conflict-target column list, MariaDB does not (`ON DUPLICATE KEY UPDATE`).
pub(crate) fn upsert_tail(
    dialect: Dialect,
    oc: &OnConflict,
    model: &RModel,
    sets: &[String],
) -> String {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => {
            format!("\nON DUPLICATE KEY UPDATE {}", sets.join(", "))
        }
        Dialect::Postgres | Dialect::Sqlite => {
            let target: Vec<String> = oc
                .target
                .iter()
                .map(|t| dialect.quote(&physical_col(model, &t.node)))
                .collect();
            format!(
                "\nON CONFLICT ({}) DO UPDATE SET {}",
                target.join(", "),
                sets.join(", ")
            )
        }
    }
}
