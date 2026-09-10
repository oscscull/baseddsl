//! Render a literal / value-position function / JSON scalar column to SQL text.

use super::*;

/// A bare single-segment variant value rendered as its wire value literal — a quoted
/// string for a string enum, a bare integer for an int enum — or `None` when `value` is
/// not a bare identifier (a `$param` binds normally; anything else falls through to
/// ordinary value lowering) or names no variant of `en`.
pub(crate) fn variant_lit(dialect: Dialect, en: &REnum, value: &Value) -> Option<String> {
    match value {
        Value::Path(vp) if vp.segments.len() == 1 => match en.wire_of(&vp.segments[0].node)? {
            EnumValue::Str(s) => Some(render_lit(dialect, &Literal::Str(s.clone()))),
            EnumValue::Int(n) => Some(n.to_string()),
        },
        _ => None,
    }
}

pub(crate) fn render_lit(dialect: Dialect, l: &Literal) -> String {
    match l {
        Literal::Str(s) => format!("'{}'", s.replace('\'', "''")),
        Literal::Int(i) => i.to_string(),
        Literal::Decimal(s) => s.clone(),
        Literal::Bool(b) => dialect.bool_lit(*b).to_string(),
        Literal::Null => "NULL".to_string(),
    }
}

pub(crate) fn render_func(f: &FuncCall) -> String {
    // `now()` is the only value-position function (ir::KNOWN_FUNCS).
    match f.name.node.as_str() {
        "now" => "CURRENT_TIMESTAMP".to_string(),
        other => format!("{other}()"),
    }
}

impl<'a> Select<'a> {
    /// One scalar column inside a JSON element body. Two families need a cast so the
    /// SQL-built JSON element matches the wire contract:
    ///   * a `decimal` — the wire carries its exact JSON *string*, but a native numeric
    ///     would render as a JSON number and lose digits (SQLite stores it as TEXT — no
    ///     cast needed);
    ///   * a `bytes` — the wire carries **base64**, but the DB's own JSON rendering of a
    ///     binary column is the wrong form (Postgres hex `\x…`, MariaDB a `base64:type…`
    ///     tag), so it is base64-encoded in SQL (`encode`/`TO_BASE64`). SQLite's JSON
    ///     functions cannot carry a `BLOB`, so a `bytes` field inside a to-many array is
    ///     unsupported there — project it flat instead.
    pub(crate) fn json_scalar(&self, alias: &str, col: &str, path: &Path, model: &RModel) -> String {
        let qcol = self.qcol(alias, col);
        match path_primitive(self.schema, model, path) {
            Primitive::Decimal { .. } => match self.dialect {
                Dialect::Postgres => format!("({qcol})::text"),
                Dialect::MariaDb | Dialect::MySql => format!("CAST({qcol} AS CHAR)"),
                Dialect::Sqlite => qcol,
            },
            Primitive::Bytes => match self.dialect {
                Dialect::Postgres => format!("encode({qcol}, 'base64')"),
                Dialect::MariaDb | Dialect::MySql => format!("TO_BASE64({qcol})"),
                Dialect::Sqlite => qcol,
            },
            _ => qcol,
        }
    }
}
