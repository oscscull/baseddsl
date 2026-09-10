//! Column DDL and neutral-type/default lowering.
//!
//! [`column_ddl`] renders one column definition; [`neutral_sql_type`] and
//! [`render_neutral_default`] map a neutral snapshot type/default to the dialect's SQL.

use super::*;

/// A column definition `<name> <type> NULL|NOT NULL [DEFAULT <lit>]`, shared by
/// `CREATE TABLE` bodies, `ADD COLUMN`, and MariaDB's `MODIFY COLUMN`. Matches
/// `sql::column_line` so an `add column` reads identically to a `create table` column.
pub(crate) fn column_ddl(c: &ColumnSnap, dialect: Dialect) -> String {
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
pub(crate) fn neutral_sql_type(neutral: &str, dialect: Dialect) -> String {
    let (base, many) = match neutral.strip_suffix("[]") {
        Some(b) => (b, true),
        None => (neutral, false),
    };
    // An opaque column's type is the literal the author wrote, used verbatim.
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
pub(crate) fn render_neutral_default(d: &str, dialect: Dialect) -> String {
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
pub(crate) fn enum_check_values(neutral: &str) -> Option<Vec<String>> {
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
/// string, or the map's entry for the target (sema guarantees it exists).
pub(crate) fn raw_body(canonical: &str, dialect: Dialect) -> String {
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
