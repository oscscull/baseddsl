//! Reverse (`down.mig`) rendering.
//!
//! [`render_down`] prefills real reverse SQL for mechanically reversible steps and a loud
//! `-- … is irreversible …` comment for the rest.

use super::*;

/// Render a prefilled `down.mig` (raw per-dialect SQL) reversing `steps`, newest step first.
/// A mechanically reversible step (`add`⇄`drop`, `rename`⇄`rename`, `create table`⇄`drop
/// table`) becomes real reverse SQL; anything that loses data or whose forward form doesn't
/// carry enough to reconstruct (a `drop`, an `alter column`, a `raw` escape) becomes a loud
/// `-- … is irreversible …` comment inviting a hand-written reverse. A `down.mig` that ends
/// up all-comment (no executable statement) is treated as absent — the migration stays
/// roll-forward only until the author completes it.
pub fn render_down(steps: &[Step], dialect: Dialect) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- down.mig — reverse of up.mig (raw {} SQL). Prefilled where mechanically reversible;",
        dialect.name()
    );
    out.push_str(
        "-- complete or delete the irreversible steps below before relying on rollback.\n",
    );
    for step in steps.iter().rev() {
        out.push('\n');
        match reverse_statements(step, dialect) {
            Some(stmts) => {
                for s in stmts {
                    let _ = writeln!(out, "{s};");
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "-- {} is irreversible (data loss); write your own or delete this file",
                    step.describe()
                );
            }
        }
    }
    out
}

/// The reverse SQL for a step, or `None` when it isn't mechanically reversible. `Some(vec![])`
/// (a scope change — no DDL either way) contributes nothing but isn't "irreversible".
fn reverse_statements(step: &Step, dialect: Dialect) -> Option<Vec<String>> {
    Some(match step {
        Step::CreateTable(t) => vec![format!(
            "DROP TABLE {}",
            dialect.quote_table(t.schema.as_deref(), &t.name)
        )],
        Step::AddColumn {
            table,
            schema,
            column,
        } => vec![format!(
            "ALTER TABLE {} DROP COLUMN {}",
            dialect.quote_table(schema.as_deref(), table),
            dialect.quote(&column.name),
        )],
        Step::AddIndex {
            table,
            schema,
            index,
        }
        | Step::AddUnique {
            table,
            schema,
            index,
        } => {
            vec![drop_index_sql(dialect, schema.as_deref(), table, &index.name)]
        }
        // An added FK reverses to a drop (safe on PG/MariaDB; SQLite has no in-place drop,
        // so its reverse is left to a hand-authored raw step — mark irreversible here).
        Step::AddForeignKey { table, schema, fk } => {
            match drop_foreign_key_statements(table, schema.as_deref(), &fk.columns, dialect) {
                Ok(stmts) => stmts,
                Err(_) => return None,
            }
        }
        Step::RenameTable { from, to, schema } => vec![format!(
            "ALTER TABLE {} RENAME TO {}",
            dialect.quote_table(schema.as_deref(), to),
            dialect.quote(from),
        )],
        Step::RenameColumn {
            table,
            schema,
            from,
            to,
        } => vec![format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            dialect.quote_table(schema.as_deref(), table),
            dialect.quote(to),
            dialect.quote(from),
        )],
        // A schema move reverses to the mirror move (to → from), mechanically reversible on
        // Postgres/MySQL; SQLite's forward form already errored, so no reverse is reached.
        Step::AlterSchema { table, from, to } => {
            alter_schema_statements(table, to, from, dialect).ok()?
        }
        // A scope change emits no DDL forward, so its reverse is likewise a no-op — present
        // but empty, and not flagged irreversible.
        Step::ScopeChange(_) => vec![],
        // Drops/alters lose the prior state; a raw escape is opaque. Not reversible.
        Step::DropTable { .. }
        | Step::DropColumn { .. }
        | Step::AlterColumn { .. }
        | Step::DropIndex { .. }
        | Step::DropUnique { .. }
        | Step::DropForeignKey { .. }
        | Step::Raw { .. } => return None,
    })
}
