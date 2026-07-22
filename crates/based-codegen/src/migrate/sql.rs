//! Render a neutral [`Step`] list to per-dialect SQL.
//!
//! [`render_sql`] is the reviewable "show me the SQL" surface; [`sql_statements`] is its
//! executable twin for `based migrate apply` (both go through the same lowering, so
//! applied SQL == reviewed SQL). [`content_hash`] anchors the `_based_migrations` ledger's
//! tamper guard. The neutral type map goes through [`crate::sql::sql_type`], so a
//! migration's DDL can never drift from `based gen sql`.

use super::diff::{ColumnChange, Step};
use super::model::{index_name, ColumnSnap, IndexSnap, TableSnap};
use super::up_mig::scope_change_line;
use crate::Dialect;
use based_ast::Primitive;
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
        // A scope change alters generated code, not the database — render it as a note.
        if let Step::ScopeChange(sc) = step {
            let _ = writeln!(
                out,
                "-- scope contract change (no DDL): {}",
                scope_change_line(sc)
            );
            continue;
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
            continue;
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
    out
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

/// Bare executable statement(s) for one neutral step (no trailing `;`, no comment). A
/// `CreateTable` yields several on SQLite/Postgres (the table + trailing `CREATE INDEX`es);
/// most steps yield one.
fn step_statements(step: &Step, dialect: Dialect) -> Result<Vec<String>, String> {
    Ok(match step {
        Step::CreateTable(t) => create_table_statements(t, dialect),
        Step::DropTable(name) => vec![format!("DROP TABLE {}", dialect.quote(name))],
        Step::AddColumn { table, column } => vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            dialect.quote(table),
            column_ddl(column, dialect),
        )],
        Step::DropColumn { table, column } => vec![format!(
            "ALTER TABLE {} DROP COLUMN {}",
            dialect.quote(table),
            dialect.quote(column),
        )],
        Step::AlterColumn {
            table,
            column,
            changes,
            after,
        } => alter_column_statements(table, column, changes, after, dialect)?,
        Step::AddIndex { table, index } | Step::AddUnique { table, index } => {
            vec![create_index_sql(dialect, table, index)]
        }
        Step::DropIndex { table, name } | Step::DropUnique { table, name } => {
            vec![drop_index_sql(dialect, table, name)]
        }
        // Renames are a safe in-place ALTER on every target (Postgres always; MariaDB
        // ≥10.5.2 / SQLite ≥3.25 for `RENAME COLUMN`; `RENAME TO` universal) — existing
        // data survives, so this is a real rename, never a drop+recreate.
        Step::RenameTable { from, to } => vec![format!(
            "ALTER TABLE {} RENAME TO {}",
            dialect.quote(from),
            dialect.quote(to),
        )],
        Step::RenameColumn { table, from, to } => vec![format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            dialect.quote(table),
            dialect.quote(from),
            dialect.quote(to),
        )],
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
            h ^= *b as u64;
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

/// The statement(s) for a full `CREATE TABLE` from a neutral snapshot table. Mirrors
/// `sql::create_table`: the implicit `id` PK is re-synthesized (it is elided from the
/// snapshot) unless the model declared its own; `(unique)` columns become `CONSTRAINT …
/// UNIQUE`; indexes are inline `KEY`/`UNIQUE KEY` on MariaDB (one statement) and trailing
/// standalone `CREATE INDEX` statements elsewhere.
fn create_table_statements(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Implicit `id` primary key: synthesized as the default uuid when the snapshot
    // elided it; a declared non-default `id` rides in the column list instead. A keyless
    // (`@no_id`) table has neither the column nor the `PRIMARY KEY`.
    if !t.no_id && t.column("id").is_none() {
        lines.push(format!(
            "{} {} NOT NULL",
            dialect.quote("id"),
            crate::sql::sql_type(Primitive::Uuid, false, dialect),
        ));
    }
    for c in &t.columns {
        lines.push(column_ddl(c, dialect));
    }
    if !t.no_id {
        lines.push(format!("PRIMARY KEY ({})", dialect.quote("id")));
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

    // MariaDB inlines indexes as table clauses; SQLite/Postgres trail them as statements.
    if dialect == Dialect::MariaDb {
        for i in &t.indexes {
            let cols = quote_cols(dialect, &i.columns);
            let kind = if i.unique { "UNIQUE KEY" } else { "KEY" };
            lines.push(format!("{kind} {} ({cols})", dialect.quote(&i.name)));
        }
    }

    let body = lines
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join(",\n");
    let mut stmts = vec![format!(
        "CREATE TABLE {} (\n{body}\n)",
        dialect.quote(&t.name)
    )];
    if dialect != Dialect::MariaDb {
        for i in &t.indexes {
            stmts.push(create_index_sql(dialect, &t.name, i));
        }
    }
    stmts
}

fn alter_column_statements(
    table: &str,
    column: &str,
    changes: &[ColumnChange],
    after: &ColumnSnap,
    dialect: Dialect,
) -> Result<Vec<String>, String> {
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
                    "ALTER TABLE {} ALTER COLUMN {} {clause}",
                    dialect.quote(table),
                    dialect.quote(column),
                )
            })
            .collect(),
        // MariaDB: a type/null change needs a full `MODIFY COLUMN` (no piecemeal form);
        // a default-only change uses `ALTER COLUMN … SET/DROP DEFAULT`.
        Dialect::MariaDb => {
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
                    "ALTER TABLE {} MODIFY COLUMN {}",
                    dialect.quote(table),
                    column_ddl(after, dialect),
                )]
            } else {
                changes
                    .iter()
                    .filter_map(|ch| match ch {
                        ColumnChange::SetDefault(d) => Some(format!(
                            "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {}",
                            dialect.quote(table),
                            dialect.quote(column),
                            render_neutral_default(d, dialect),
                        )),
                        ColumnChange::DropDefault => Some(format!(
                            "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT",
                            dialect.quote(table),
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
fn create_index_sql(dialect: Dialect, table: &str, index: &IndexSnap) -> String {
    let kind = if index.unique {
        "CREATE UNIQUE INDEX"
    } else {
        "CREATE INDEX"
    };
    format!(
        "{kind} {} ON {} ({})",
        dialect.quote(&index.name),
        dialect.quote(table),
        quote_cols(dialect, &index.columns),
    )
}

/// `DROP INDEX` — MySQL/MariaDB require the `ON <table>` qualifier; SQLite/Postgres
/// drop by index name alone. Bare (no trailing `;`).
fn drop_index_sql(dialect: Dialect, table: &str, name: &str) -> String {
    match dialect {
        Dialect::MariaDb => format!(
            "DROP INDEX {} ON {}",
            dialect.quote(name),
            dialect.quote(table)
        ),
        Dialect::Sqlite | Dialect::Postgres => format!("DROP INDEX {}", dialect.quote(name)),
    }
}

/// A column definition `<name> <type> NULL|NOT NULL [DEFAULT <lit>]`, shared by
/// `CREATE TABLE` bodies, `ADD COLUMN`, and MariaDB's `MODIFY COLUMN`. Matches
/// `sql::column_line` so an `add column` reads identically to a `create table` column.
fn column_ddl(c: &ColumnSnap, dialect: Dialect) -> String {
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
    let prim = match base {
        "text" => Primitive::Text,
        "int" => Primitive::Int,
        "bool" => Primitive::Bool,
        "timestamp" => Primitive::Timestamp,
        "date" => Primitive::Date,
        "json" => Primitive::Json,
        "uuid" => Primitive::Uuid,
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

/// Quote a physical column list for the dialect, comma-joined.
fn quote_cols(dialect: Dialect, cols: &[String]) -> String {
    cols.iter()
        .map(|c| dialect.quote(c))
        .collect::<Vec<_>>()
        .join(", ")
}
