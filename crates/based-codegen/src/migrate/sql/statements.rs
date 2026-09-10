//! Neutral step → bare per-dialect statement(s).
//!
//! [`step_statements`] routes each [`Step`] to its DDL builder; `CreateTable` and the
//! `ALTER`/foreign-key/index builders live here. Statements are bare (no trailing `;`).

use super::*;

/// Bare executable statement(s) for one neutral step (no trailing `;`, no comment). A
/// `CreateTable` yields several on SQLite/Postgres (the table + trailing `CREATE INDEX`es);
/// most steps yield one.
pub(crate) fn step_statements(step: &Step, dialect: Dialect) -> Result<Vec<String>, String> {
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
        // data survives, so this is a real rename rather than a drop+recreate.
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

/// The statement(s) for a full `CREATE TABLE` from a neutral snapshot table. Mirrors
/// `sql::create_table`: the implicit `id` PK is re-synthesized (it is elided from the
/// snapshot) unless the model declared its own; `(unique)` columns become `CONSTRAINT …
/// UNIQUE`; indexes are inline `KEY`/`UNIQUE KEY` on MariaDB (one statement) and trailing
/// standalone `CREATE INDEX` statements elsewhere.
pub(crate) fn create_table_statements(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.extend(pk_and_column_lines(t, dialect));
    lines.extend(unique_constraint_lines(t, dialect));
    lines.extend(enum_check_lines(t, dialect));
    lines.extend(fk_constraint_lines(t, dialect));
    // MySQL/MariaDB inline indexes as table clauses; SQLite/Postgres trail them as statements.
    if dialect.is_mysql_family() {
        lines.extend(inline_index_lines(t, dialect));
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
    // An opaque `raw` index always trails as a standalone statement, even on MySQL/MariaDB.
    for i in t
        .indexes
        .iter()
        .filter(|i| !dialect.is_mysql_family() || i.raw.is_some())
    {
        stmts.push(create_index_sql(dialect, schema, &t.name, i));
    }
    stmts
}

/// The primary-key and column body lines, in create order: the synthesized `id`, then each
/// declared column (a `serial` carries its auto-increment/identity clause), then the
/// `PRIMARY KEY` clause.
fn pk_and_column_lines(t: &TableSnap, dialect: Dialect) -> Vec<String> {
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
    lines
}

/// Column-level `(unique)` constraint lines (a declared `@index (unique)` is an IndexSnap
/// instead — emitted as an index — so there is no double-emit).
fn unique_constraint_lines(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    t.columns
        .iter()
        .filter(|c| c.unique)
        .map(|c| {
            format!(
                "CONSTRAINT {} UNIQUE ({})",
                dialect.quote(&index_name("uq", &t.name, std::slice::from_ref(&c.name))),
                dialect.quote(&c.name),
            )
        })
        .collect()
}

/// Enum CHECK-constraint lines — the same DB-native form `based gen sql` emits, so a
/// from-scratch migration matches the generated DDL.
fn enum_check_lines(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    t.columns
        .iter()
        .filter_map(|c| {
            enum_check_values(&c.ty)
                .map(|values| crate::sql::enum_check_clause(dialect, &t.name, &c.name, &values))
        })
        .collect()
}

/// Inline foreign-key constraint lines — works on all three targets (SQLite too), so a
/// from-scratch migration builds the same FKs `based gen sql` emits.
fn fk_constraint_lines(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    t.foreign_keys
        .iter()
        .map(|fk| crate::sql::fk_constraint_clause(dialect, &t.name, fk))
        .collect()
}

/// MySQL/MariaDB inline index lines (`KEY`/`UNIQUE KEY`/`FULLTEXT KEY`/`SPATIAL KEY`), one
/// per non-raw index.
fn inline_index_lines(t: &TableSnap, dialect: Dialect) -> Vec<String> {
    t.indexes
        .iter()
        .filter(|i| i.raw.is_none())
        .map(|i| {
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
            format!("{kind} {} ({cols}){using}", dialect.quote(&i.name))
        })
        .collect()
}

/// `ALTER TABLE … ADD CONSTRAINT … FOREIGN KEY …` on Postgres/MariaDB. SQLite cannot
/// ALTER-add an FK (it requires the 12-step table rebuild the neutral vocabulary can't
/// safely auto-generate) — surface a loud, greppable message pointing at a hand-authored
/// `raw(sqlite)` rebuild rather than a silently-skipped constraint.
fn add_foreign_key_statements(
    table: &str,
    schema: Option<&str>,
    fk: &ForeignKeySnap,
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
pub(crate) fn alter_schema_statements(
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
pub(crate) fn drop_foreign_key_statements(
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

/// The `ALTER COLUMN` statement(s) for a column change. Postgres emits one `ALTER COLUMN`
/// sub-statement per change; MySQL/MariaDB need a full `MODIFY COLUMN` for a type/null
/// change and `ALTER COLUMN … SET/DROP DEFAULT` for a default-only one; SQLite has none.
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
        Dialect::MariaDb | Dialect::MySql => alter_column_mysql(&table_q, column, changes, after, dialect),
        // SQLite has no in-place ALTER COLUMN — a type/null/default change requires the
        // 12-step table rebuild, which the neutral vocabulary can't safely auto-generate.
        // Surface a loud, greppable message pointing at a hand-authored raw(sqlite) step
        // rather than broken SQL.
        Dialect::Sqlite => {
            return Err(format!(
                "SQLite cannot ALTER COLUMN {table}.{column} in place; author a raw(sqlite) table-rebuild migration."
            ))
        }
    })
}

/// The MySQL/MariaDB `ALTER COLUMN` lowering: a full `MODIFY COLUMN` when the change touches
/// type or nullability, else one `SET`/`DROP DEFAULT` per default change.
fn alter_column_mysql(
    table_q: &str,
    column: &str,
    changes: &[ColumnChange],
    after: &ColumnSnap,
    dialect: Dialect,
) -> Vec<String> {
    let structural = changes.iter().any(|c| {
        matches!(
            c,
            ColumnChange::Type { .. } | ColumnChange::SetNull | ColumnChange::SetNotNull { .. }
        )
    });
    if structural {
        return vec![format!(
            "ALTER TABLE {table_q} MODIFY COLUMN {}",
            column_ddl(after, dialect),
        )];
    }
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
pub(crate) fn drop_index_sql(
    dialect: Dialect,
    schema: Option<&str>,
    table: &str,
    name: &str,
) -> String {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => format!(
            "DROP INDEX {} ON {}",
            dialect.quote(name),
            dialect.quote_table(schema, table)
        ),
        Dialect::Sqlite | Dialect::Postgres => format!("DROP INDEX {}", dialect.quote(name)),
    }
}
