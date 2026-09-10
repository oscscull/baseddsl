//! Per-dialect type + enum + foreign-key lowering. One primitive-to-SQL map,
//! shared with the migration renderer so a from-scratch migration matches
//! `based gen sql`.

use super::*;

/// SQL column type for a scalar primitive, per dialect. A to-many scalar has no columnar
/// form, so it is stored as a JSON array — the dialect's JSON type (`JSON` on MariaDB,
/// `TEXT` on SQLite, `JSONB` on Postgres). `pub(crate)` so the migration renderer maps
/// neutral snapshot types through this one map, keeping the two in lockstep.
pub(crate) fn sql_type(ty: Primitive, many: bool, dialect: Dialect) -> String {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => mysql_sql_type(ty, many),
        Dialect::Sqlite => sqlite_sql_type(ty, many),
        Dialect::Postgres => postgres_sql_type(ty, many),
    }
}

/// The MySQL/MariaDB family. `uuid`/`Id` is `CHAR(36)` — it holds the app-minted v4
/// string exactly and executes on every version (MySQL has no `UUID` type; MariaDB added
/// it only in 10.7). Native `UUID` (MariaDB 10.7+) stays reachable via `raw`.
fn mysql_sql_type(ty: Primitive, many: bool) -> String {
    if many {
        return "JSON".into();
    }
    match ty {
        Primitive::Text => "VARCHAR(255)".into(),
        Primitive::Int => "BIGINT".into(),
        Primitive::Bool => "BOOLEAN".into(),
        Primitive::Timestamp => "DATETIME".into(),
        Primitive::Date => "DATE".into(),
        Primitive::Time => "TIME".into(),
        // A binary blob — `BLOB` (no length; up to 64 KiB, the family's plain variable
        // binary). A bigger payload takes a raw `MEDIUMBLOB`/`LONGBLOB`.
        Primitive::Bytes => "BLOB".into(),
        Primitive::Json => "JSON".into(),
        Primitive::Uuid | Primitive::Id => "CHAR(36)".into(),
        // `ulid` is a 26-char Crockford-base32 string — a fixed CHAR.
        Primitive::Ulid => "CHAR(26)".into(),
        // `serial`'s storage type — a plain `BIGINT`; the `AUTO_INCREMENT` clause rides
        // the PK column definition, and FK columns mirror this.
        Primitive::Serial => "BIGINT".into(),
        Primitive::Float => "DOUBLE".into(),
        Primitive::Decimal { precision, scale } => format!("DECIMAL({precision}, {scale})"),
    }
}

/// SQLite has a tiny type set (its dynamic typing means these are affinities). The mapping
/// mirrors the runtime `SqliteDb` value mapping: bool ⇒ integer 0/1, json/uuid/timestamp/date
/// ⇒ text. `decimal` takes TEXT affinity so it is stored + returned as its exact string, no
/// digit lost and the wire form stays a JSON string as it is on MariaDB/Postgres. (Comparison
/// is therefore lexicographic on SQLite — a fixed affinity limitation; production dialects use
/// a true numeric DECIMAL.)
fn sqlite_sql_type(ty: Primitive, many: bool) -> String {
    if many {
        return "TEXT".into();
    }
    match ty {
        Primitive::Text => "TEXT".into(),
        Primitive::Int | Primitive::Bool => "INTEGER".into(),
        // SQLite has no native TIME — store the canonical `HH:MM:SS` string as `TEXT` (the
        // same degradation `date`/`timestamp`/`decimal` take here).
        Primitive::Timestamp | Primitive::Date | Primitive::Time => "TEXT".into(),
        Primitive::Bytes => "BLOB".into(),
        Primitive::Json => "TEXT".into(),
        Primitive::Uuid | Primitive::Id | Primitive::Ulid => "TEXT".into(),
        // `serial`'s storage type (the `PRIMARY KEY AUTOINCREMENT` rides the column).
        Primitive::Serial => "INTEGER".into(),
        Primitive::Float => "REAL".into(),
        Primitive::Decimal { .. } => "TEXT".into(),
    }
}

/// Postgres has a rich, standards-track type set: real `BOOLEAN`, native `UUID`, tz-aware
/// `TIMESTAMPTZ`, and `JSONB` (the indexable/`@>`-queryable JSON form — matching the DML `has`
/// -> `@>` lowering). `text` is unbounded (no VARCHAR cap).
fn postgres_sql_type(ty: Primitive, many: bool) -> String {
    if many {
        return "JSONB".into();
    }
    match ty {
        Primitive::Text => "TEXT".into(),
        Primitive::Int => "BIGINT".into(),
        Primitive::Bool => "BOOLEAN".into(),
        Primitive::Timestamp => "TIMESTAMPTZ".into(),
        Primitive::Date => "DATE".into(),
        Primitive::Time => "TIME".into(),
        Primitive::Bytes => "BYTEA".into(),
        Primitive::Json => "JSONB".into(),
        Primitive::Uuid | Primitive::Id => "UUID".into(),
        // `ulid` is a 26-char string — store as text.
        Primitive::Ulid => "TEXT".into(),
        // `serial`'s storage type (the `GENERATED … AS IDENTITY` clause rides the PK column
        // definition; FK columns mirror this plain `BIGINT`).
        Primitive::Serial => "BIGINT".into(),
        Primitive::Float => "DOUBLE PRECISION".into(),
        Primitive::Decimal { precision, scale } => format!("NUMERIC({precision}, {scale})"),
    }
}

/// A single enum wire value rendered as a SQL literal: a string quoted, an int bare.
pub(crate) fn enum_value_sql(v: &based_sema::EnumValue) -> String {
    match v {
        based_sema::EnumValue::Str(s) => format!("'{}'", s.replace('\'', "''")),
        based_sema::EnumValue::Int(n) => n.to_string(),
    }
}

/// The `CONSTRAINT ck_<table>_<col> CHECK (<col> IN (v1, …))` clause enforcing an enum
/// column's values (each already rendered as a SQL literal — `'paid'` or `0`). Shared
/// with the migration renderer so a from-scratch migration matches `based gen sql`.
pub(crate) fn enum_check_clause(
    dialect: Dialect,
    table: &str,
    column: &str,
    values: &[String],
) -> String {
    let list = values.join(", ");
    format!(
        "CONSTRAINT {name} CHECK ({col} IN ({list}))",
        name = dialect.quote(&constraint_name(
            "ck",
            table,
            std::slice::from_ref(&column.to_string())
        )),
        col = dialect.quote(column),
    )
}

/// A `CONSTRAINT fk_<table>_<col> FOREIGN KEY (<col>) REFERENCES <ref>(<ref_col>) [ON
/// DELETE <a>] [ON UPDATE <a>]` clause. Valid inline on all three dialects (SQLite honors
/// an inline table FK); shared by `based gen sql` and the migration create-table renderer,
/// so a from-scratch migration matches the generated DDL.
pub(crate) fn fk_constraint_clause(
    dialect: Dialect,
    table: &str,
    fk: &crate::migrate::ForeignKeySnap,
) -> String {
    let name = constraint_name("fk", table, &fk.columns);
    let quote_list = |cols: &[String]| {
        cols.iter()
            .map(|c| dialect.quote(c))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut s = format!(
        "CONSTRAINT {name} FOREIGN KEY ({col}) REFERENCES {ref_table} ({ref_col})",
        name = dialect.quote(&name),
        col = quote_list(&fk.columns),
        ref_table = dialect.quote_table(fk.ref_schema.as_deref(), &fk.ref_table),
        ref_col = quote_list(&fk.ref_columns),
    );
    if let Some(a) = &fk.on_delete {
        s.push_str(&format!(" ON DELETE {}", fk_action_sql(a)));
    }
    if let Some(a) = &fk.on_update {
        s.push_str(&format!(" ON UPDATE {}", fk_action_sql(a)));
    }
    s
}

/// The SQL clause for a neutral referential-action spelling (`cascade` → `CASCADE`, …). A
/// hand-edited/unknown value rides through uppercased (parse/verify guards this upstream).
pub(crate) fn fk_action_sql(a: &str) -> String {
    match based_sema::FkAction::parse(a) {
        Some(act) => act.sql().to_string(),
        None => a.to_uppercase().replace('_', " "),
    }
}

/// The full column definition for a `serial` (DB-generated sequential) primary key, from
/// one neutral spelling: MySQL/MariaDB `BIGINT NOT NULL AUTO_INCREMENT`, Postgres `BIGINT
/// GENERATED ALWAYS AS IDENTITY`, SQLite `INTEGER PRIMARY KEY AUTOINCREMENT` (SQLite's
/// only native auto-increment form is inline on the column, so the PK clause is omitted
/// for it — see `create_table`). Shared with the migration create-table renderer so a
/// from-scratch migration matches `based gen sql`.
pub(crate) fn serial_pk_column(dialect: Dialect, column: &str) -> String {
    let col = dialect.quote(column);
    match dialect {
        Dialect::MariaDb | Dialect::MySql => format!("{col} BIGINT NOT NULL AUTO_INCREMENT"),
        Dialect::Postgres => format!("{col} BIGINT GENERATED ALWAYS AS IDENTITY"),
        Dialect::Sqlite => format!("{col} INTEGER PRIMARY KEY AUTOINCREMENT"),
    }
}

/// The SQL type of a relation's FK column — the target model's primary-key type.
/// A PK with a `raw` type (e.g. native `UUID`) propagates that literal so the escape
/// hatch composes across the FK. Defaults to the dialect's id type (from the implicit
/// `id`) when the target or its key is missing, which sema would already have flagged.
fn fk_type(schema: &CheckedSchema, target: &str, dialect: Dialect) -> String {
    let id_kind = schema
        .model(target)
        .and_then(RModel::pk_member)
        .map(|m| &m.kind);
    if let Some(spec) = id_kind.and_then(|k| k.opaque()) {
        return spec
            .for_dialect(dialect.name())
            .unwrap_or_default()
            .to_string();
    }
    match id_kind {
        Some(MemberKind::Scalar { ty, .. }) => sql_type(*ty, false, dialect),
        _ => sql_type(Primitive::Uuid, false, dialect),
    }
}

/// The SQL type of a primary-key part column — used to type the FK column(s) that mirror
/// it. A scalar part carries its own (raw or primitive) type; a relation part (a junction
/// key `@key(order, product)`) carries its own target's key type, so the mirror composes.
pub(crate) fn key_part_sql_type(schema: &CheckedSchema, part: &RMember, dialect: Dialect) -> String {
    match &part.kind {
        MemberKind::Scalar {
            raw_type: Some(spec),
            ..
        } => spec
            .for_dialect(dialect.name())
            .unwrap_or_default()
            .to_string(),
        MemberKind::Scalar { ty, .. } => sql_type(*ty, false, dialect),
        MemberKind::Forward { target, .. } => fk_type(schema, target, dialect),
        MemberKind::Inverse { .. } => sql_type(Primitive::Uuid, false, dialect),
    }
}
