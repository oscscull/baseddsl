//! The SQLite table-rebuild lowering.
//!
//! SQLite lacks in-place `ALTER COLUMN` / foreign-key changes, so [`sqlite_plan`] folds every
//! step touching such a table into one recreate → copy → swap rebuild at the target shape.

use super::*;

/// Lower a step list for SQLite: group every step touching a table that needs a rebuild
/// into one [`Emit::Rebuild`] (emitted at the table's first touch, in step order), and pass
/// every other step through as [`Emit::Step`]. A rebuild is triggered only when the target
/// snapshot actually carries the table's post-migration shape — otherwise the step falls
/// through to its per-step lowering (which surfaces the honest "can't in place" error).
pub(crate) fn sqlite_plan<'a>(steps: &'a [Step], target: &Snapshot) -> Vec<Emit<'a>> {
    let rebuild: Vec<&str> = steps
        .iter()
        .filter_map(rebuild_trigger_table)
        .filter(|t| target.table(t).is_some())
        .collect();
    let mut plan = Vec::new();
    let mut done: HashSet<&str> = HashSet::new();
    for step in steps {
        if let Some(t) = folded_table(step) {
            if rebuild.contains(&t) {
                if done.insert(t) {
                    plan.push(Emit::Rebuild {
                        table: t.to_string(),
                        stmts: sqlite_rebuild_statements(t, target, steps),
                    });
                }
                continue;
            }
        }
        plan.push(Emit::Step(step));
    }
    plan
}

/// The table a step forces a SQLite rebuild of (an in-place edit SQLite lacks), else `None`.
fn rebuild_trigger_table(step: &Step) -> Option<&str> {
    match step {
        Step::AlterColumn { table, .. }
        | Step::AddForeignKey { table, .. }
        | Step::DropForeignKey { table, .. }
        | Step::AlterSchema { table, .. } => Some(table),
        _ => None,
    }
}

/// The existing table a step touches, i.e. one a rebuild of that table subsumes. `None` for
/// steps that create/drop/rename a whole table or emit no DDL — those are folded separately.
fn folded_table(step: &Step) -> Option<&str> {
    match step {
        Step::AddColumn { table, .. }
        | Step::DropColumn { table, .. }
        | Step::AlterColumn { table, .. }
        | Step::AddIndex { table, .. }
        | Step::DropIndex { table, .. }
        | Step::AddUnique { table, .. }
        | Step::DropUnique { table, .. }
        | Step::AddForeignKey { table, .. }
        | Step::DropForeignKey { table, .. }
        | Step::RenameColumn { table, .. }
        | Step::AlterSchema { table, .. } => Some(table),
        Step::CreateTable(_)
        | Step::DropTable { .. }
        | Step::RenameTable { .. }
        | Step::Raw { .. }
        | Step::ScopeChange(_) => None,
    }
}

/// The SQLite 12-step table rebuild for `table`, recreating it at its `target` shape and
/// copying the surviving rows. Follows the procedure SQLite documents: `legacy_alter_table`
/// keeps the rename from re-pointing other tables' FKs, `defer_foreign_keys` holds FK checks
/// to the (apply-owned) commit so the transient swap doesn't trip them. Columns added in this
/// migration have no source and are left to their default; a renamed column copies from its
/// old name; a schema move copies out of the old namespace into the new.
fn sqlite_rebuild_statements(table: &str, target: &Snapshot, steps: &[Step]) -> Vec<String> {
    let Some(tt) = target.table(table) else {
        return Vec::new();
    };
    let d = Dialect::Sqlite;

    let added = rebuild_added_columns(table, steps);
    let (old_name, source_schema) = rebuild_renames(table, tt, steps);
    let (tcols, scols) = rebuild_copy_lists(tt, &added, &old_name, d);

    // Reuse the from-scratch create so the rebuilt table is byte-identical to `gen sql`; its
    // trailing `CREATE INDEX`es run after the swap (indexes die with the dropped old table).
    let mut create = create_table_statements(tt, d);
    let create_table = create.remove(0);

    let tmp = format!("{table}__based_rebuild");
    let source = d.quote_table(source_schema.as_deref(), table);
    let tmp_q = d.quote_table(source_schema.as_deref(), &tmp);
    let target_q = d.quote_table(tt.schema.as_deref(), table);

    let mut stmts = vec![
        "PRAGMA legacy_alter_table = ON".to_string(),
        "PRAGMA defer_foreign_keys = ON".to_string(),
        format!("ALTER TABLE {source} RENAME TO {}", d.quote(&tmp)),
        create_table,
    ];
    if !tcols.is_empty() {
        stmts.push(format!(
            "INSERT INTO {target_q} ({}) SELECT {} FROM {tmp_q}",
            tcols.join(", "),
            scols.join(", "),
        ));
    }
    stmts.push(format!("DROP TABLE {tmp_q}"));
    stmts.extend(create);
    stmts.push("PRAGMA legacy_alter_table = OFF".to_string());
    stmts
}

/// The columns introduced by this migration on `table` — they have no source data, so a
/// rebuild leaves them to their default rather than copying.
fn rebuild_added_columns<'a>(table: &str, steps: &'a [Step]) -> HashSet<&'a str> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::AddColumn {
                table: t, column, ..
            } if t == table => Some(column.name.as_str()),
            _ => None,
        })
        .collect()
}

/// The rebuild's source lookup for `table`: each renamed column's old name (target → old),
/// and the schema the surviving rows are copied out of (the pre-move namespace).
fn rebuild_renames<'a>(
    table: &str,
    tt: &'a TableSnap,
    steps: &'a [Step],
) -> (HashMap<&'a str, &'a str>, Option<String>) {
    let mut old_name: HashMap<&str, &str> = HashMap::new();
    let mut source_schema = tt.schema.clone();
    for s in steps {
        match s {
            Step::RenameColumn {
                table: t, from, to, ..
            } if t == table => {
                old_name.insert(to.as_str(), from.as_str());
            }
            Step::AlterSchema { table: t, from, .. } if t == table => {
                source_schema.clone_from(from);
            }
            _ => {}
        }
    }
    (old_name, source_schema)
}

/// The rebuild's `INSERT … SELECT` column lists: the columns the target keeps (its
/// synthesized `id` included), each paired with its source name — a renamed column copies
/// from its old name. Columns introduced this migration have no source, so they are skipped.
fn rebuild_copy_lists(
    tt: &TableSnap,
    added: &HashSet<&str>,
    old_name: &HashMap<&str, &str>,
    d: Dialect,
) -> (Vec<String>, Vec<String>) {
    let (mut tcols, mut scols) = (Vec::new(), Vec::new());
    for c in physical_columns(tt) {
        if added.contains(c.as_str()) {
            continue;
        }
        let src_col = old_name.get(c.as_str()).copied().unwrap_or(c.as_str());
        tcols.push(d.quote(&c));
        scols.push(d.quote(src_col));
    }
    (tcols, scols)
}

/// The physical column names of a table in create order — the synthesized default `id`
/// (elided from the snapshot) first when the model didn't nominate its own key, then the
/// declared columns. Mirrors [`create_table_statements`]'s column set so a rebuild's copy
/// list lines up with the recreated table.
fn physical_columns(t: &TableSnap) -> Vec<String> {
    let mut cols = Vec::new();
    if !t.no_id && t.pk.is_empty() && t.column("id").is_none() {
        cols.push("id".to_string());
    }
    cols.extend(t.columns.iter().map(|c| c.name.clone()));
    cols
}
