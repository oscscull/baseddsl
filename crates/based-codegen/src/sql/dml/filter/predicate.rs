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
                let (alias, col) = self.resolve(path, model);
                format!(
                    "{} = {}",
                    self.qcol(&alias, &col),
                    self.dialect.bool_lit(true)
                )
            }
            // Inline the filter's body, substituting args for its params, resolved
            // against the call-site model (sema guarantees the body resolves).
            Predicate::FilterCall { name, args } => match self.filters.get(name.node.as_str()) {
                Some(f) => self.filter_call(f, args, model),
                None => format!("TRUE /* filter {} unresolved */", name.node),
            },
            Predicate::Raw(raw) => format!(
                "({})",
                render_raw(self.dialect, raw, &self.root_alias, &model.table)
            ),
        }
    }

    /// One comparison leaf `path op value`. An optional context read, a null test, an enum
    /// variant RHS, and the collection ops each take their own form; the whole leaf is then
    /// present-guarded when its RHS is a `?` optional param.
    fn cmp(&mut self, path: &Path, op: Op, value: &Value, model: &RModel) -> String {
        let (alias, col) = self.resolve(path, model);
        let lhs = self.qcol(&alias, &col);
        // Optional context read `$ctx.field?` (auth.md Handle 1): an absent field is
        // SQL NULL, so `=`/`!=` against it lower to null-safe (in)equality — absent
        // matches the rows whose own column is unset (`col IS NULL`) rather than
        // widening the filter. (A non-`=` operator keeps a plain comparison, where a
        // NULL bind simply matches nothing.) No present-guard: the null is the signal.
        if is_optional_ctx_read(value) && matches!(op, Op::Eq | Op::Ne) {
            let rhs = self.value(value, model);
            return nullsafe_cmp(self.dialect, &lhs, &rhs, op == Op::Ne);
        }
        let pred = if matches!(value, Value::Lit(Literal::Null)) && matches!(op, Op::Eq | Op::Ne) {
            // `col = null` / `col != null` are null tests, not value comparisons:
            // SQL `col = NULL` is always NULL (never matches), so lower to IS [NOT] NULL.
            let nullness = if op == Op::Eq {
                "IS NULL"
            } else {
                "IS NOT NULL"
            };
            format!("{lhs} {nullness}")
        } else {
            // An enum column compares against a bare variant, which lowers to its
            // wire string literal (not a column reference).
            let rhs = self
                .enum_variant_lit(model, path, value)
                .unwrap_or_else(|| self.value(value, model));
            match op {
                // Collection ops don't fit plain infix: `in` needs a value list,
                // `has` is JSON-array containment — MySQL's `value MEMBER OF(arr)`
                // vs. Postgres's `arr @> value` (the JSONB containment operator).
                Op::In => format!("{lhs} IN ({rhs})"),
                Op::Has => match self.dialect {
                    Dialect::Postgres => format!("{lhs} @> {rhs}"),
                    _ => format!("{rhs} MEMBER OF({lhs})"),
                },
                _ => format!("{lhs} {} {rhs}", sql_op(op)),
            }
        };
        // A `?` optional param on the RHS present-guards this leaf so it drops when
        // the arg is absent — the piece that makes an or-composed optional filter work.
        self.guard_optional(value, pred)
    }

    /// `path in (v, v, …)`: each element renders as an equality RHS would.
    fn in_list(&mut self, path: &Path, values: &[Value], model: &RModel) -> String {
        let (alias, col) = self.resolve(path, model);
        let lhs = self.qcol(&alias, &col);
        let mut items = Vec::with_capacity(values.len());
        for v in values {
            let item = self
                .enum_variant_lit(model, path, v)
                .unwrap_or_else(|| self.value(v, model));
            items.push(item);
        }
        format!("{lhs} IN ({})", items.join(", "))
    }
}
