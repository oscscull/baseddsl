//! Resolve a `Value` (param / tx-binding / path / incoming) to SQL, and the placeholder
//! naming its params and bindings bind through.

use super::super::*;

/// `$ctx.org` -> `ctx_org`; `$id` -> `id`. Placeholder-safe (dots removed).
pub(crate) fn param_key(pr: &ParamRef) -> String {
    let mut k = pr.name.node.clone();
    for seg in &pr.path {
        k.push('_');
        k.push_str(&seg.node);
    }
    k
}

/// The bind name a `$name.field` reference reads from — the value the bound create's
/// row read-back captured for physical column `column`. One placeholder per
/// (binding, column); the run stage binds it from the re-selected row.
pub(crate) fn bref_name(binding: &str, column: &str) -> String {
    format!("bref_{binding}__{column}")
}

impl<'a> Select<'a> {
    pub(crate) fn value(&mut self, v: &Value, model: &RModel) -> String {
        match v {
            // A `$name.field` that names a reachable tx step binding resolves to that
            // step's produced row; any other `$…` is an ordinary bound parameter.
            Value::Param(pr) if self.bindings.contains_key(pr.name.node.as_str()) => {
                self.binding_value(pr)
            }
            Value::Param(pr) => format!(":{}", param_key(pr)),
            // `incoming.<col>` in a bulk upsert's SET clause: the proposed/incoming row's
            // value — `excluded.<col>` (Postgres/SQLite) / `VALUES(<col>)` (MySQL/MariaDB).
            Value::Path(p)
                if self.incoming && p.segments.len() == 2 && p.segments[0].node == "incoming" =>
            {
                let col = physical_col(model, &p.segments[1].node);
                match self.dialect {
                    Dialect::Postgres | Dialect::Sqlite => format!("excluded.{}", self.q(&col)),
                    Dialect::MariaDb | Dialect::MySql => format!("VALUES({})", self.q(&col)),
                }
            }
            Value::Path(p) => {
                let (alias, col) = self.resolve(p, model);
                if self.bare_cols {
                    self.q(&col)
                } else {
                    self.qcol(&alias, &col)
                }
            }
            Value::Lit(l) => render_lit(self.dialect, l),
            Value::Func(f) => render_func(f),
        }
    }

    /// Lower a `$name.field` tx step reference to the bind holding the bound create's
    /// re-selected value for that field. Sema (E0281) guarantees the binding and the
    /// field resolve.
    fn binding_value(&self, pr: &ParamRef) -> String {
        let field = pr.path.first().map_or("", |s| s.node.as_str());
        self.binding_field_value(&pr.name.node, field)
    }

    /// The bind a `$name.field` reference reads from: the bound create re-selects its
    /// written row, and this is the placeholder holding that row's value for `field`'s
    /// physical column — the one uniform path for every field (id, scalar, `@scope`
    /// column, engine timestamp, DB default). Reused by a composite-FK assign to pull
    /// each key part.
    pub(super) fn binding_field_value(&self, name: &str, field: &str) -> String {
        let column = self
            .schema
            .model(self.bindings[name].model)
            .map_or_else(|| field.to_string(), |m| physical_col(m, field));
        format!(":{}", bref_name(name, &column))
    }
}
