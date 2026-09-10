//! Render a raw-SQL escape-hatch fragment to SQL text.

use super::*;

/// Render a raw-SQL fragment: text verbatim, `${param}` -> `:param`,
/// `{table}`/`{id}` -> safe engine interpolation (root table / its `id`). Only the
/// engine-interpolated identifiers are dialect-quoted; the raw text is the user's and
/// is emitted verbatim (an escape hatch — they own its portability).
pub(crate) fn render_raw(dialect: Dialect, raw: &RawSql, root_alias: &str, table: &str) -> String {
    let mut s = String::new();
    for part in &raw.parts {
        match part {
            RawPart::Text(t) => s.push_str(t),
            RawPart::Param(pr) => s.push_str(&format!(":{}", param_key(pr))),
            RawPart::Engine(id) => match id.node.as_str() {
                "table" => s.push_str(&dialect.quote(table)),
                "id" => s.push_str(&dialect.qcol(root_alias, "id")),
                other => s.push_str(&dialect.quote(other)),
            },
        }
    }
    s
}
