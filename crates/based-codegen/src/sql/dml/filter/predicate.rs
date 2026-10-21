//! Render a predicate tree (a `where` / a CASE `when`) to comparison SQL.

use super::super::*;
use super::optional::{is_optional_ctx_read, nullsafe_cmp};

impl<'a> Select<'a> {
    pub(crate) fn predicate(&mut self, p: &Predicate, model: &RModel) -> String {
        match p {
            Predicate::And(a, b) => {
                format!(
                    "({} AND {})",
                    self.predicate(a, model),
                    self.predicate(b, model)
                )
            }
            Predicate::Or(a, b) => {
                format!(
                    "({} OR {})",
                    self.predicate(a, model),
                    self.predicate(b, model)
                )
            }
            Predicate::Not(inner) => format!("NOT ({})", self.predicate(inner, model)),
            Predicate::Cmp { path, op, value } => self.cmp(path, *op, value, model),
            // `path in (v, v, …)`: each element lowers like an equality RHS — an
            // enum variant to its wire value, a `$param` to its own placeholder
            // (bound positionally), a literal per-dialect.
            Predicate::InList { path, values } => self.in_list(path, values, model),
            // A bare atom is either a zero-arg named filter or a plain bool column.
            Predicate::Bare(path) => {
                if path.segments.len() == 1 {
                    if let Some(f) = self.filters.get(path.segments[0].node.as_str()).copied() {
                        return self.filter_call(f, &[], model);
                    }
                }
                let truth = self.dialect.bool_lit(true);
                self.leaf(path, model, |lhs| format!("{lhs} = {truth}"))
            }
            // Inline the filter's body, substituting args for its params, resolved
            // against the call-site model (sema guarantees the body resolves).
            Predicate::FilterCall { name, args } => match self.filters.get(name.node.as_str()) {
                Some(f) => self.filter_call(f, args, model),
                None => format!("TRUE /* filter {} unresolved */", name.node),
            },
            Predicate::Any { path, pred, .. } => self.relation_any(path, pred.as_deref(), model),
            Predicate::Raw(raw) => format!(
                "({})",
                render_raw(self.dialect, raw, &self.root_alias, Some(model))
            ),
        }
    }

    /// One comparison leaf `path op value`. An optional context read, a null test, an enum
    /// variant RHS, and the collection ops each take their own form; the whole leaf is then
    /// present-guarded when its RHS is a `?` optional param.
    fn cmp(&mut self, path: &Path, op: Op, value: &Value, model: &RModel) -> String {
        let rhs = self.rhs(path, value, model);
        let dialect = self.dialect;
        let pred = self.leaf(path, model, |lhs| compare(dialect, lhs, op, value, &rhs));
        // A `?` optional param on the RHS present-guards this leaf so it drops when
        // the arg is absent — the piece that makes an or-composed optional filter work.
        self.guard_optional(value, pred)
    }

    /// `path in (v, v, …)`: each element renders as an equality RHS would.
    fn in_list(&mut self, path: &Path, values: &[Value], model: &RModel) -> String {
        let items: Vec<String> = values.iter().map(|v| self.rhs(path, v, model)).collect();
        self.leaf(path, model, |lhs| {
            format!("{lhs} IN ({})", items.join(", "))
        })
    }

    /// A comparison's right operand: an enum column's bare variant as its wire literal,
    /// anything else as an ordinary value.
    fn rhs(&mut self, path: &Path, value: &Value, model: &RModel) -> String {
        self.enum_variant_lit(model, path, value)
            .unwrap_or_else(|| self.value(value, model))
    }
}

/// `lhs op rhs` for one comparison leaf.
fn compare(dialect: Dialect, lhs: &str, op: Op, value: &Value, rhs: &str) -> String {
    // Optional context read `$ctx.field?` (auth.md Handle 1): an absent field is SQL NULL,
    // so `=`/`!=` lower to null-safe (in)equality — absent matches the rows whose own
    // column is unset rather than widening the filter.
    if is_optional_ctx_read(value) && matches!(op, Op::Eq | Op::Ne) {
        return nullsafe_cmp(dialect, lhs, rhs, op == Op::Ne);
    }
    // `col = null` / `col != null` are null tests: SQL `col = NULL` never matches.
    if matches!(value, Value::Lit(Literal::Null)) && matches!(op, Op::Eq | Op::Ne) {
        let nullness = if op == Op::Eq {
            "IS NULL"
        } else {
            "IS NOT NULL"
        };
        return format!("{lhs} {nullness}");
    }
    match op {
        // `in` needs a value list; `has` is JSON-array containment — MySQL's
        // `value MEMBER OF(arr)` vs. Postgres's `arr @> value`.
        Op::In => format!("{lhs} IN ({rhs})"),
        Op::Has => match dialect {
            Dialect::Postgres => format!("{lhs} @> {rhs}"),
            _ => format!("{rhs} MEMBER OF({lhs})"),
        },
        _ => format!("{lhs} {} {rhs}", sql_op(op)),
    }
}
