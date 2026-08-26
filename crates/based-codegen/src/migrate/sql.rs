//! Render a neutral [`Step`] list to per-dialect SQL.
//!
//! [`render_sql`] is the reviewable "show me the SQL" surface; [`sql_statements`] is its
//! executable twin for `based migrate apply` (both go through the same lowering, so
//! applied SQL == reviewed SQL). [`content_hash`] anchors the `_based_migrations` ledger's
//! tamper guard. The neutral type map goes through [`crate::sql::sql_type`], so a
//! migration's DDL can never drift from `based gen sql`.

use super::diff::{strip_raw_steps, ColumnChange, Step};
use super::model::{index_name, ColumnSnap, IndexSnap, Snapshot, TableSnap};
use super::up_mig::{render_up, scope_change_line};
use crate::Dialect;
use based_ast::Primitive;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

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
        // a loud, greppable comment, never broken SQL.
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
enum Emit<'a> {
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

/// Lower a step list for SQLite: group every step touching a table that needs a rebuild
/// into one [`Emit::Rebuild`] (emitted at the table's first touch, in step order), and pass
/// every other step through as [`Emit::Step`]. A rebuild is triggered only when the target
/// snapshot actually carries the table's post-migration shape — otherwise the step falls
/// through to its per-step lowering (which surfaces the honest "can't in place" error).
fn sqlite_plan<'a>(steps: &'a [Step], target: &Snapshot) -> Vec<Emit<'a>> {
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
/// steps that create/drop/rename a whole table or emit no DDL — those are never folded.
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

    let added: HashSet<&str> = steps
        .iter()
        .filter_map(|s| match s {
            Step::AddColumn {
                table: t, column, ..
            } if t == table => Some(column.name.as_str()),
            _ => None,
        })
        .collect();
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

    // Copy the columns the target keeps (its synthesized `id` included), each from its old
    // name; skip columns introduced this migration (no source data).
    let (mut tcols, mut scols) = (Vec::new(), Vec::new());
    for c in physical_columns(tt) {
        if added.contains(c.as_str()) {
            continue;
        }
        let src_col = old_name.get(c.as_str()).copied().unwrap_or(c.as_str());
        tcols.push(d.quote(&c));
        scols.push(d.quote(src_col));
    }

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

/// Bare executable statement(s) for one neutral step (no trailing `;`, no comment). A
/// `CreateTable` yields several on SQLite/Postgres (the table + trailing `CREATE INDEX`es);
/// most steps yield one.
fn step_statements(step: &Step, dialect: Dialect) -> Result<Vec<String>, String> {
    Ok(match step {
        Step::CreateTable(t) => create_table_statements(t, dialect),
        Step::DropTable { name, schema } => vec![format!(
            "DROP TABLE {}",
            dialect.quote_table(schema.as_deref(), name)
        )],
        Step::AddColumn {
            table,
            schema,
            column,
        } => vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            dialect.quote_table(schema.as_deref(), table),
            column_ddl(column, dialect),
        )],
        Step::DropColumn {
            table,
            schema,
            column,
        } => vec![format!(
            "ALTER TABLE {} DROP COLUMN {}",
            dialect.quote_table(schema.as_deref(), table),
            dialect.quote(column),
        )],
        Step::AlterColumn {
            table,
            schema,
            column,
            changes,
            after,
        } => alter_column_statements(table, schema.as_deref(), column, changes, after, dialect)?,
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
            vec![create_index_sql(dialect, schema.as_deref(), table, index)]
        }
        Step::DropIndex {
            table,
            schema,
            name,
        }
        | Step::DropUnique {
            table,
            schema,
            name,
        } => {
            vec![drop_index_sql(dialect, schema.as_deref(), table, name)]
        }
        Step::AddForeignKey { table, schema, fk } => {
            add_foreign_key_statements(table, schema.as_deref(), fk, dialect)?
        }
        Step::DropForeignKey {
            table,
            schema,
            columns,
        } => drop_foreign_key_statements(table, schema.as_deref(), columns, dialect)?,
        // Renames are a safe in-place ALTER on every target (Postgres always; MariaDB
        // ≥10.5.2 / SQLite ≥3.25 for `RENAME COLUMN`; `RENAME TO` universal) — existing
        // data survives, so this is a real rename, never a drop+recreate.
        Step::RenameTable { from, to, schema } => vec![format!(
            "ALTER TABLE {} RENAME TO {}",
            dialect.quote_table(schema.as_deref(), from),
            // `RENAME TO` names the new table unqualified — it stays in the same schema.
            dialect.quote(to),
        )],
        Step::RenameColumn {
            table,
            schema,
            from,
            to,
        } => vec![format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            dialect.quote_table(schema.as_deref(), table),
            dialect.quote(from),
            dialect.quote(to),
        )],
        Step::AlterSchema { table, from, to } => alter_schema_statements(table, from, to, dialect)?,
        // A raw escape runs verbatim only when its dialect matches the target; for any
        // other dialect it is a no-op here (its per-dialect twin carries that target).
        Step::Raw { dialect: d, sql } => {
            if *d == dialect {
                vec![sql.clone()]
            } else {
                vec![]
            }
        }
        // A scope change is code-level (an injected filter), not DDL — no SQL to run.
        Step::ScopeChange(_) => vec![],
    })
}

/// A stable content hash of an `up.mig`'s canonical bytes — the `_based_migrations`
/// ledger's tamper guard. Canonicalization drops comment (`#…`) and blank
/// lines and trims each remaining line, so a cosmetic whitespace/comment edit doesn't trip
/// the guard but any change to a step does. FNV-1a-64 (the same family the runtime uses for
/// request fingerprints), rendered as 16 lowercase hex digits — collision resistance
/// is not security-critical here (it guards against an accidental post-apply edit, not an
/// adversary), so a fast non-cryptographic hash is the right tool.
pub fn content_hash(up_text: &str) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for line in up_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        mix(line.as_bytes());
        mix(b"\n");
    }
    format!("{h:016x}")
}

/// Does the stored `up.mig`'s structural (non-`raw`) residue still match the steps its
/// `schema.snap` chain implies? The snapshot-authoritative drift check shared by `apply`,
/// `render`, and `verify`: structural steps derive from `schema.snap`, so a hand-edit that
/// changes a structural line's canonical form is drift and must be refused rather than
/// silently ignored. Canonicalized like [`content_hash`] (cosmetic whitespace/comment edits
/// are tolerated; `raw(dialect)` escapes ride separately and are stripped before compare).
pub fn up_mig_matches_snapshot(up_text: &str, steps: &[Step]) -> bool {
    content_hash(&strip_raw_steps(up_text)) == content_hash(&render_up(steps))
}

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
            vec![drop_index_sql(
                dialect,
                schema.as_deref(),
                table,
                &index.name,
            )]
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
        // but empty, never flagged irreversible.
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

/// The statement(s) for a full `CREATE TABLE` from a neutral snapshot table. Mirrors
/// `sql::create_table`: the implicit `id` PK is re-synthesized (it is elided from the
/// snapshot) unless the model declared its own; `(unique)` columns become `CONSTRAINT …
/// UNIQUE`; indexes are inline `KEY`/`UNIQUE KEY` on MariaDB (one statement) and trailing
/// standalone `CREATE INDEX` statements elsewhere.
fn create_table_statements(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Primary key: its physical column(s) are the `pk=` marker, else the default `id`. The
    // default `id` is elided from the snapshot, so re-synthesize it as the default uuid; a
    // renamed `id`, a single-column `@key`, or a composite `@key`'s columns ride in the
    // column list instead. A keyless (`@no_id`) table has neither the column nor the clause.
    let pk_cols: Vec<String> = if t.pk.is_empty() {
        vec!["id".to_string()]
    } else {
        t.pk.clone()
    };
    if !t.no_id && t.pk.is_empty() && t.column("id").is_none() {
        lines.push(format!(
            "{} {} NOT NULL",
            dialect.quote("id"),
            crate::sql::sql_type(Primitive::Uuid, false, dialect),
        ));
    }
    // A `serial` primary key carries the dialect's auto-increment/identity clause on its
    // column (SQLite spells it inline as `INTEGER PRIMARY KEY AUTOINCREMENT`, so it needs
    // no separate PK clause). The serial column is never elided from the snapshot.
    let serial_pk = t.columns.iter().any(|c| c.ty == "serial");
    for c in &t.columns {
        if c.ty == "serial" {
            lines.push(crate::sql::serial_pk_column(dialect, &c.name));
        } else {
            lines.push(column_ddl(c, dialect));
        }
    }
    let sqlite_serial = dialect == Dialect::Sqlite && serial_pk;
    if !t.no_id && !sqlite_serial {
        let cols = pk_cols
            .iter()
            .map(|c| dialect.quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("PRIMARY KEY ({cols})"));
    }

    // Column-level `(unique)` constraints (a declared `@index (unique)` is an IndexSnap
    // instead — handled below — so there is no double-emit).
    for c in &t.columns {
        if c.unique {
            lines.push(format!(
                "CONSTRAINT {} UNIQUE ({})",
                dialect.quote(&index_name("uq", &t.name, std::slice::from_ref(&c.name))),
                dialect.quote(&c.name),
            ));
        }
    }

    // Enum columns carry a CHECK constraint on their variants — the same DB-native form
    // `based gen sql` emits, so a from-scratch migration matches the generated DDL.
    for c in &t.columns {
        if let Some(values) = enum_check_values(&c.ty) {
            lines.push(crate::sql::enum_check_clause(
                dialect, &t.name, &c.name, &values,
            ));
        }
    }

    // Foreign-key constraints — inline in the create (works on all three, SQLite too), so a
    // from-scratch migration builds the same FKs `based gen sql` emits.
    for fk in &t.foreign_keys {
        lines.push(crate::sql::fk_constraint_clause(dialect, &t.name, fk));
    }

    // MySQL/MariaDB inline indexes as table clauses; SQLite/Postgres trail them as statements.
    // An opaque `raw` index is never a table clause — it always trails.
    if dialect.is_mysql_family() {
        for i in t.indexes.iter().filter(|i| i.raw.is_none()) {
            let cols = quote_cols(dialect, &i.columns);
            let (kind, using) = match i.method.as_deref() {
                Some("fulltext") => ("FULLTEXT KEY", String::new()),
                Some("spatial") => ("SPATIAL KEY", String::new()),
                Some(m) => (
                    if i.unique { "UNIQUE KEY" } else { "KEY" },
                    format!(" USING {}", m.to_uppercase()),
                ),
                None => (if i.unique { "UNIQUE KEY" } else { "KEY" }, String::new()),
            };
            lines.push(format!("{kind} {} ({cols}){using}", dialect.quote(&i.name)));
        }
    }

    let body = lines
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join(",\n");
    let schema = t.schema.as_deref();
    let mut stmts = vec![format!(
        "CREATE TABLE {} (\n{body}\n)",
        dialect.quote_table(schema, &t.name)
    )];
    for i in t
        .indexes
        .iter()
        .filter(|i| !dialect.is_mysql_family() || i.raw.is_some())
    {
        stmts.push(create_index_sql(dialect, schema, &t.name, i));
    }
    stmts
}

/// `ALTER TABLE … ADD CONSTRAINT … FOREIGN KEY …` on Postgres/MariaDB. SQLite cannot
/// ALTER-add an FK (it requires the 12-step table rebuild the neutral vocabulary can't
/// safely auto-generate) — surface a loud, greppable message pointing at a hand-authored
/// `raw(sqlite)` rebuild, never a silently-skipped constraint.
fn add_foreign_key_statements(
    table: &str,
    schema: Option<&str>,
    fk: &super::model::ForeignKeySnap,
    dialect: Dialect,
) -> Result<Vec<String>, String> {
    if dialect == Dialect::Sqlite {
        return Err(format!(
            "SQLite cannot ALTER TABLE {table} ADD the foreign key on `{}`; author a raw(sqlite) table-rebuild migration.",
            fk.label()
        ));
    }
    let name = index_name("fk", table, &fk.columns);
    let quote_list = |cols: &[String]| {
        cols.iter()
            .map(|c| dialect.quote(c))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut s = format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        dialect.quote_table(schema, table),
        dialect.quote(&name),
        quote_list(&fk.columns),
        dialect.quote_table(fk.ref_schema.as_deref(), &fk.ref_table),
        quote_list(&fk.ref_columns),
    );
    if let Some(a) = &fk.on_delete {
        let _ = write!(s, " ON DELETE {}", crate::sql::fk_action_sql(a));
    }
    if let Some(a) = &fk.on_update {
        let _ = write!(s, " ON UPDATE {}", crate::sql::fk_action_sql(a));
    }
    Ok(vec![s])
}

/// Move a table between SQL schemas (Postgres) / databases (MySQL/MariaDB). Postgres has an
/// in-place `ALTER TABLE … SET SCHEMA`; the MySQL family expresses it as a cross-database
/// `RENAME TABLE from_db.t TO to_db.t`. SQLite cannot move a table across attached databases
/// in place, so it surfaces a loud raw-rebuild pointer rather than a silently-skipped move.
fn alter_schema_statements(
    table: &str,
    from: &Option<String>,
    to: &Option<String>,
    dialect: Dialect,
) -> Result<Vec<String>, String> {
    let from_q = dialect.quote_table(from.as_deref(), table);
    match dialect {
        Dialect::Postgres => {
            let target = to.as_deref().unwrap_or("public");
            Ok(vec![format!(
                "ALTER TABLE {from_q} SET SCHEMA {}",
                dialect.quote(target)
            )])
        }
        Dialect::MariaDb | Dialect::MySql => {
            let to_q = dialect.quote_table(to.as_deref(), table);
            Ok(vec![format!("RENAME TABLE {from_q} TO {to_q}")])
        }
        Dialect::Sqlite => Err(format!(
            "SQLite cannot move {table} across attached databases in place; author a raw(sqlite) table-rebuild migration."
        )),
    }
}

/// `ALTER TABLE … DROP CONSTRAINT` (Postgres) / `DROP FOREIGN KEY` (MariaDB). SQLite has no
/// in-place FK drop either — same honest table-rebuild message.
fn drop_foreign_key_statements(
    table: &str,
    schema: Option<&str>,
    columns: &[String],
    dialect: Dialect,
) -> Result<Vec<String>, String> {
    let name = index_name("fk", table, columns);
    Ok(match dialect {
        Dialect::Postgres => vec![format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            dialect.quote_table(schema, table),
            dialect.quote(&name),
        )],
        Dialect::MariaDb | Dialect::MySql => vec![format!(
            "ALTER TABLE {} DROP FOREIGN KEY {}",
            dialect.quote_table(schema, table),
            dialect.quote(&name),
        )],
        Dialect::Sqlite => {
            return Err(format!(
                "SQLite cannot ALTER TABLE {table} DROP the foreign key on `{}`; author a raw(sqlite) table-rebuild migration.",
                columns.join(", ")
            ))
        }
    })
}

fn alter_column_statements(
    table: &str,
    schema: Option<&str>,
    column: &str,
    changes: &[ColumnChange],
    after: &ColumnSnap,
    dialect: Dialect,
) -> Result<Vec<String>, String> {
    let table_q = dialect.quote_table(schema, table);
    Ok(match dialect {
        // Postgres: one `ALTER COLUMN` sub-statement per change (it has them all).
        Dialect::Postgres => changes
            .iter()
            .map(|ch| {
                let clause = match ch {
                    ColumnChange::Type { to, .. } => {
                        format!("TYPE {}", neutral_sql_type(to, dialect))
                    }
                    ColumnChange::SetNull => "DROP NOT NULL".to_string(),
                    ColumnChange::SetNotNull { .. } => "SET NOT NULL".to_string(),
                    ColumnChange::SetDefault(d) => {
                        format!("SET DEFAULT {}", render_neutral_default(d, dialect))
                    }
                    ColumnChange::DropDefault => "DROP DEFAULT".to_string(),
                };
                format!(
                    "ALTER TABLE {table_q} ALTER COLUMN {} {clause}",
                    dialect.quote(column),
                )
            })
            .collect(),
        // MySQL/MariaDB: a type/null change needs a full `MODIFY COLUMN` (no piecemeal form);
        // a default-only change uses `ALTER COLUMN … SET/DROP DEFAULT`.
        Dialect::MariaDb | Dialect::MySql => {
            let structural = changes.iter().any(|c| {
                matches!(
                    c,
                    ColumnChange::Type { .. }
                        | ColumnChange::SetNull
                        | ColumnChange::SetNotNull { .. }
                )
            });
            if structural {
                vec![format!(
                    "ALTER TABLE {table_q} MODIFY COLUMN {}",
                    column_ddl(after, dialect),
                )]
            } else {
                changes
                    .iter()
                    .filter_map(|ch| match ch {
                        ColumnChange::SetDefault(d) => Some(format!(
                            "ALTER TABLE {table_q} ALTER COLUMN {} SET DEFAULT {}",
                            dialect.quote(column),
                            render_neutral_default(d, dialect),
                        )),
                        ColumnChange::DropDefault => Some(format!(
                            "ALTER TABLE {table_q} ALTER COLUMN {} DROP DEFAULT",
                            dialect.quote(column),
                        )),
                        _ => None,
                    })
                    .collect()
            }
        }
        // SQLite has no in-place ALTER COLUMN — a type/null/default change requires the
        // 12-step table rebuild, which the neutral vocabulary can't safely auto-generate.
        // Surface a loud, greppable message pointing at a hand-authored raw(sqlite) step
        // (the escape hatch is never silent) rather than broken SQL.
        Dialect::Sqlite => {
            return Err(format!(
                "SQLite cannot ALTER COLUMN {table}.{column} in place; author a raw(sqlite) table-rebuild migration."
            ))
        }
    })
}

/// A standalone `CREATE [UNIQUE] INDEX` (all dialects share this form for an add). Bare
/// (no trailing `;`); `render_sql` terminates it, `apply` executes it as-is.
fn create_index_sql(
    dialect: Dialect,
    schema: Option<&str>,
    table: &str,
    index: &IndexSnap,
) -> String {
    let kind = if index.unique {
        "CREATE UNIQUE INDEX"
    } else {
        "CREATE INDEX"
    };
    let name = dialect.quote(&index.name);
    let table_q = dialect.quote_table(schema, table);
    // An opaque index's body replaces the column list verbatim.
    if let Some(raw) = &index.raw {
        return format!("{kind} {name} ON {table_q} {}", raw_body(raw, dialect));
    }
    let cols = quote_cols(dialect, &index.columns);
    match index.method.as_deref() {
        Some(m) => format!("{kind} {name} ON {table_q} USING {m} ({cols})"),
        None => format!("{kind} {name} ON {table_q} ({cols})"),
    }
}

/// `DROP INDEX` — MySQL/MariaDB require the `ON <table>` qualifier (namespaced when the
/// table lives in a named database); SQLite/Postgres drop by index name alone (Postgres
/// resolves the index in its own schema, so no table qualifier is needed). Bare.
fn drop_index_sql(dialect: Dialect, schema: Option<&str>, table: &str, name: &str) -> String {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => format!(
            "DROP INDEX {} ON {}",
            dialect.quote(name),
            dialect.quote_table(schema, table)
        ),
        Dialect::Sqlite | Dialect::Postgres => format!("DROP INDEX {}", dialect.quote(name)),
    }
}

/// A column definition `<name> <type> NULL|NOT NULL [DEFAULT <lit>]`, shared by
/// `CREATE TABLE` bodies, `ADD COLUMN`, and MariaDB's `MODIFY COLUMN`. Matches
/// `sql::column_line` so an `add column` reads identically to a `create table` column.
fn column_ddl(c: &ColumnSnap, dialect: Dialect) -> String {
    // A generated column carries its `GENERATED ALWAYS AS (<expr>) STORED` clause instead of a
    // nullability/default. The snapshot stores the expression in dialect-neutral DSL, so it is
    // re-parsed and re-lowered here per target dialect — concat becomes `||` on Postgres/SQLite
    // and `CONCAT(…)` on the MySQL family, exactly like `based gen sql`. A snapshot too corrupt
    // to parse falls back to the stored text verbatim (a hand-edit; parse/verify guards it).
    if let Some(g) = &c.generated {
        let expr_sql = based_parser::parse_expr(g, based_ast::FileId(0)).map_or_else(
            |_| g.clone(),
            |expr| crate::sql::generated_expr(None, None, &expr, crate::sql::Emit::Sql(dialect)),
        );
        return format!(
            "{} {} GENERATED ALWAYS AS ({expr_sql}) STORED",
            dialect.quote(&c.name),
            neutral_sql_type(&c.ty, dialect),
        );
    }
    let mut s = format!(
        "{} {} {}",
        dialect.quote(&c.name),
        neutral_sql_type(&c.ty, dialect),
        if c.nullable { "NULL" } else { "NOT NULL" },
    );
    if let Some(d) = &c.default {
        let _ = write!(s, " DEFAULT {}", render_neutral_default(d, dialect));
    }
    s
}

/// Map a neutral snapshot type (`int`/`text`/`uuid`/…, `[]` for a to-many scalar) to the
/// dialect's SQL type — through `sql::sql_type`, the *same* map `based gen sql` uses.
fn neutral_sql_type(neutral: &str, dialect: Dialect) -> String {
    let (base, many) = match neutral.strip_suffix("[]") {
        Some(b) => (b, true),
        None => (neutral, false),
    };
    // An opaque column's type is the literal the author wrote — never a mapped primitive.
    if neutral.starts_with("raw(") {
        return raw_body(neutral, dialect);
    }
    let prim = match base {
        "text" => Primitive::Text,
        "int" => Primitive::Int,
        "bool" => Primitive::Bool,
        "timestamp" => Primitive::Timestamp,
        "date" => Primitive::Date,
        "time" => Primitive::Time,
        "bytes" => Primitive::Bytes,
        "json" => Primitive::Json,
        "uuid" => Primitive::Uuid,
        "ulid" => Primitive::Ulid,
        "serial" => Primitive::Serial,
        "float" => Primitive::Float,
        // `decimal(p,s)` carries its precision/scale so the renderer emits the exact
        // `DECIMAL(p,s)`/`NUMERIC(p,s)` — a precision/scale change is a real column-type diff.
        b if b.starts_with("decimal(") => parse_decimal(b),
        // An enum column is stored as text (`enum(v1,…)`) or an integer
        // (`enum:int(0,…)`); its values ride a CHECK the create-table renderer adds
        // (see `enum_check_values`).
        b if b.starts_with("enum:int(") => Primitive::Int,
        b if b.starts_with("enum(") => Primitive::Text,
        // A corrupt/hand-edited snapshot type; parse/verify guards this upstream.
        _ => Primitive::Text,
    };
    crate::sql::sql_type(prim, many, dialect)
}

/// Parse a `decimal(p,s)` neutral snapshot type back to its `Primitive`. A malformed
/// token (a hand-edited snapshot; parse/verify guards this) falls back to the bare default.
fn parse_decimal(s: &str) -> Primitive {
    let default = Primitive::Decimal {
        precision: 38,
        scale: 9,
    };
    let Some(inner) = s.strip_prefix("decimal(").and_then(|x| x.strip_suffix(')')) else {
        return default;
    };
    let mut parts = inner.split(',');
    match (
        parts.next().and_then(|p| p.trim().parse::<u32>().ok()),
        parts.next().and_then(|p| p.trim().parse::<u32>().ok()),
    ) {
        (Some(precision), Some(scale)) => Primitive::Decimal { precision, scale },
        _ => default,
    }
}

/// Render a neutral snapshot default (`render_default`'s output — a quoted string,
/// number, `true`/`false`, `null`, or `now()`) to a dialect SQL literal/expression.
/// The inverse of `render_default`, over the same value forms.
fn render_neutral_default(d: &str, dialect: Dialect) -> String {
    // A quoted string default → a SQL string literal (unescape `\"`, then `'`-quote).
    if let Some(inner) = d.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        let unescaped = inner.replace("\\\"", "\"");
        return format!("'{}'", unescaped.replace('\'', "''"));
    }
    match d {
        "true" => dialect.bool_lit(true).to_string(),
        "false" => dialect.bool_lit(false).to_string(),
        "null" => "NULL".to_string(),
        // `now()` is the only value-position function (ir::KNOWN_FUNCS).
        _ if d.ends_with("()") => "CURRENT_TIMESTAMP".to_string(),
        // A numeric literal rides through verbatim.
        _ => d.to_string(),
    }
}

/// The CHECK value list of a neutral enum type, each already a SQL literal — `'pending'`
/// for a string enum (`enum(v1,…)`), a bare `0` for an int enum (`enum:int(0,…)`) — or
/// `None` for a non-enum column type. Inverse of `model::enum_neutral_type`.
fn enum_check_values(neutral: &str) -> Option<Vec<String>> {
    if let Some(inner) = neutral
        .strip_prefix("enum:int(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Some(inner.split(',').map(|s| s.trim().to_string()).collect());
    }
    let inner = neutral.strip_prefix("enum(")?.strip_suffix(')')?;
    Some(
        inner
            .split(',')
            .map(|s| format!("'{}'", s.trim().replace('\'', "''")))
            .collect(),
    )
}

/// The literal a canonical `raw(…)` body carries for this dialect: the bare form's one
/// string, or the map's entry for the target (sema guarantees it exists — `E0270`).
fn raw_body(canonical: &str, dialect: Dialect) -> String {
    let Some(inner) = canonical
        .strip_prefix("raw(")
        .and_then(|s| s.strip_suffix(')'))
    else {
        return canonical.to_string();
    };
    let inner = inner.trim();
    if let Some(map) = inner.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let want = format!("{}:", dialect.name());
        for entry in split_map_entries(map) {
            let entry = entry.trim();
            if let Some(v) = entry.strip_prefix(&want) {
                return unquote_snap(v.trim());
            }
        }
        return String::new();
    }
    unquote_snap(inner)
}

/// Split a canonical `raw({ a: "x, y", b: "z" })` map body on the commas that separate
/// entries — never on one inside a quoted literal.
fn split_map_entries(map: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in map.chars() {
        match c {
            _ if esc => {
                cur.push(c);
                esc = false;
            }
            '\\' if in_str => {
                cur.push(c);
                esc = true;
            }
            '"' => {
                in_str = !in_str;
                cur.push(c);
            }
            ',' if !in_str => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Unescape a snapshot string literal (`"…"` with `\"`/`\\` escapes).
fn unquote_snap(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) else {
        return s.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Quote a physical column list for the dialect, comma-joined.
fn quote_cols(dialect: Dialect, cols: &[String]) -> String {
    cols.iter()
        .map(|c| dialect.quote(c))
        .collect::<Vec<_>>()
        .join(", ")
}
