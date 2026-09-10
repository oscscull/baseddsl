//! Render a neutral [`Step`] list to per-dialect SQL.
//!
//! [`render_sql`] is the reviewable surface; [`sql_statements`] is its executable twin for
//! `based migrate apply` (both share one lowering, so applied SQL == reviewed SQL).
//! [`content_hash`] anchors the `_based_migrations` ledger's tamper guard.

use super::diff::{strip_raw_steps, ColumnChange, Step};
use super::model::{index_name, ColumnSnap, ForeignKeySnap, IndexSnap, Snapshot, TableSnap};
use super::up_mig::{render_up, scope_change_line};
use crate::Dialect;
use based_ast::Primitive;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

mod column;
mod down;
mod hash;
mod snap_parse;
mod sqlite_rebuild;
mod statements;

pub use down::render_down;
pub use hash::{content_hash, up_mig_matches_snapshot};

pub(crate) use column::*;
pub(crate) use snap_parse::*;
pub(crate) use sqlite_rebuild::*;
pub(crate) use statements::*;

// ---------- per-dialect SQL rendering --------------------------------------

/// Render a neutral step list to executable per-dialect SQL over the `Dialect` seam.
/// This is the "review the SQL" surface (`based migrate render`):
/// `0001_init`'s create steps render to the same DDL `based gen sql` builds from scratch
/// (the neutral type map goes through `sql::sql_type`, so the two can't drift).
/// A destructive step is preceded by a loud `-- DESTRUCTIVE` comment.
///
/// Deliberate dialect divergences: MariaDB alters a column with a full `MODIFY COLUMN`
/// (it has no piecemeal `SET NOT NULL`); Postgres emits one `ALTER COLUMN` per change;
/// SQLite has no in-place `ALTER COLUMN` at all, so such a step renders as a loud comment
/// pointing at a hand-authored `raw(sqlite)` table-rebuild (the neutral vocabulary's
/// edge). `DROP INDEX` also differs (MySQL/MariaDB need `ON <table>`).
pub fn render_sql(steps: &[Step], dialect: Dialect) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- Rendered by `based migrate render` (dialect: {}). Review before apply.",
        dialect.name()
    );
    for step in steps {
        out.push('\n');
        render_step_into(&mut out, step, dialect);
    }
    out
}

/// Render one step into the review text (a scope note / raw escape / `-- DESTRUCTIVE`
/// prefix / lowered statements / a loud comment for an in-place edit the dialect lacks).
/// Shared by [`render_sql`] and [`render_migration`] so the two agree per step.
fn render_step_into(out: &mut String, step: &Step, dialect: Dialect) {
    // A scope change alters generated code, not the database — render it as a note.
    if let Step::ScopeChange(sc) = step {
        let _ = writeln!(
            out,
            "-- scope contract change (no DDL): {}",
            scope_change_line(sc)
        );
        return;
    }
    // A raw escape: emit its SQL only for the matching target, else a note (its
    // per-dialect twin carries the change there). Always flagged not-verifiable.
    if let Step::Raw { dialect: d, sql } = step {
        if *d == dialect {
            let _ = writeln!(out, "-- raw({}) escape — not offline-verifiable", d.name());
            let _ = writeln!(out, "{sql};");
        } else {
            let _ = writeln!(
                out,
                "-- raw({}) step — skipped for target {}",
                d.name(),
                dialect.name()
            );
        }
        return;
    }
    if step.destructive() {
        out.push_str(
            "-- DESTRUCTIVE: needs --allow-destructive or an unsafe(\"reason\") ack to apply.\n",
        );
    }
    match step_statements(step, dialect) {
        // Each bare statement is written `;`-terminated for the reviewer/psql/mysql.
        Ok(stmts) => {
            for s in stmts {
                let _ = writeln!(out, "{s};");
            }
        }
        // A step with no in-place rendering for this dialect (SQLite `ALTER COLUMN`):
        // a loud, greppable comment rather than broken SQL.
        Err(msg) => {
            let _ = writeln!(out, "-- {msg}");
        }
    }
}

/// The executable statements for a step list, for `based migrate apply` — bare (no
/// trailing `;`, no comments), so a driver can run each through `Db::execute`. `Err(msg)`
/// = a step the dialect can't render in place (a SQLite `ALTER COLUMN` — the author must
/// supply a `raw(sqlite)` rebuild); apply surfaces it loudly rather than emit broken SQL.
/// This is the execution twin of [`render_sql`]'s review text; both go
/// through [`step_statements`], so the SQL applied is exactly the SQL reviewed.
pub fn sql_statements(steps: &[Step], dialect: Dialect) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for step in steps {
        out.extend(step_statements(step, dialect)?);
    }
    Ok(out)
}

// ---------- migration-level rendering (SQLite table rebuild) ---------------

/// One unit of a SQLite-lowered migration: either a neutral step to lower per-step, or a
/// pre-expanded table rebuild (a bundle of bare statements) that stands in for every
/// in-place change SQLite can't make to one table.
pub(crate) enum Emit<'a> {
    Step(&'a Step),
    Rebuild { table: String, stmts: Vec<String> },
}

/// The executable statements for a migration, `target` carrying its post-migration shape.
/// The migration-level twin of [`sql_statements`]: identical on Postgres/MySQL, but on
/// SQLite it expands any change that has no in-place `ALTER` (an `ALTER COLUMN`, a
/// foreign-key add/drop, a schema move) into the canonical **table rebuild** — recreate the
/// table at its target shape, copy the rows, swap it in — instead of failing loudly. This is
/// what `based migrate apply` runs on SQLite.
pub fn migration_sql(
    steps: &[Step],
    dialect: Dialect,
    target: &Snapshot,
) -> Result<Vec<String>, String> {
    if dialect != Dialect::Sqlite {
        return sql_statements(steps, dialect);
    }
    let mut out = Vec::new();
    for emit in sqlite_plan(steps, target) {
        match emit {
            Emit::Step(s) => out.extend(step_statements(s, dialect)?),
            Emit::Rebuild { stmts, .. } => out.extend(stmts),
        }
    }
    Ok(out)
}

/// The review text for a migration, `target` carrying its post-migration shape. The
/// migration-level twin of [`render_sql`]: on SQLite a rebuilt table renders as a labelled
/// block of the rebuild statements (recreate → copy → swap), so the reviewed SQL is exactly
/// what `apply` runs; other dialects delegate straight to [`render_sql`].
pub fn render_migration(steps: &[Step], dialect: Dialect, target: &Snapshot) -> String {
    if dialect != Dialect::Sqlite {
        return render_sql(steps, dialect);
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- Rendered by `based migrate render` (dialect: {}). Review before apply.",
        dialect.name()
    );
    for emit in sqlite_plan(steps, target) {
        out.push('\n');
        match emit {
            Emit::Step(s) => render_step_into(&mut out, s, dialect),
            Emit::Rebuild { table, stmts } => {
                let _ = writeln!(
                    out,
                    "-- SQLite table rebuild for `{table}`: no in-place ALTER COLUMN / foreign-key\n\
                     -- change exists, so the table is recreated at its target shape and rows copied.\n\
                     -- Runs inside the migration transaction (apply wraps it); children re-bind by name.",
                );
                for s in stmts {
                    let _ = writeln!(out, "{s};");
                }
            }
        }
    }
    out
}
