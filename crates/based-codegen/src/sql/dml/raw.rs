//! Render a raw-SQL escape-hatch fragment to SQL text.

use super::*;

/// Render a raw-SQL fragment: text verbatim, `${param}` -> `:param`,
/// `{table}`/`{id}` -> safe engine interpolation (the row model's `@schema`-qualified table / its physical
/// key column, qualified by `row_alias`). Only the engine-interpolated identifiers are
/// dialect-quoted; the raw text is the user's and is emitted verbatim. `model` is `None`
/// for a raw write, which has no row to expand placeholders against.
pub(crate) fn render_raw(
    dialect: Dialect,
    raw: &RawSql,
    row_alias: &str,
    model: Option<&RModel>,
) -> String {
    let mut s = String::new();
    for part in &raw.parts {
        match part {
            RawPart::Text(t) => s.push_str(t),
            RawPart::Param(pr) => s.push_str(&format!(":{}", param_key(pr))),
            RawPart::Engine(id) => match id.node.as_str() {
                "table" => s.push_str(&model.map_or_else(
                    || dialect.quote(""),
                    |m| dialect.quote_table(m.schema.as_deref(), &m.table),
                )),
                "id" => {
                    let key = model.map_or_else(|| "id".to_string(), pk_col);
                    s.push_str(&dialect.qcol(row_alias, &key));
                }
                other => s.push_str(&dialect.quote(other)),
            },
        }
    }
    s
}
