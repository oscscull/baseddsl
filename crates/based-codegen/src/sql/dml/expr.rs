//! Lower a computed shape-field expression to a per-row SQL scalar.

use super::*;

impl<'a> Select<'a> {
    /// Lower a computed shape-field expression to a per-row SQL scalar: arithmetic to a
    /// parenthesized `(a op b)`, concatenation through the dialect's `concat` seam, and a
    /// conditional to `CASE WHEN … THEN … ELSE … END` (its `when` reusing the shared
    /// predicate lowering, its branches recursing). Operands resolve like any reach, so a
    /// dotted path materializes the same join a `Path` projection would.
    pub(crate) fn shape_expr(
        &mut self,
        expr: &'a ShapeExpr,
        alias: &str,
        prefix: &str,
        model: &'a RModel,
    ) -> String {
        match expr {
            ShapeExpr::Value(Value::Path(p)) => {
                let (a, col) = self.resolve_from(p, alias, prefix, model);
                self.qcol(&a, &col)
            }
            ShapeExpr::Value(v) => self.value(v, model),
            ShapeExpr::Arith { lhs, op, rhs, .. } => format!(
                "({} {} {})",
                self.shape_expr(lhs, alias, prefix, model),
                arith_op_sql(*op),
                self.shape_expr(rhs, alias, prefix, model)
            ),
            ShapeExpr::Concat { .. } => {
                // Flatten a left-associative concat chain into one `a || b || c` /
                // `CONCAT(a, b, c)` rather than nesting a call per operator.
                let mut parts = Vec::new();
                self.collect_concat(expr, alias, prefix, model, &mut parts);
                self.dialect.concat(&parts)
            }
            ShapeExpr::Case { arms, else_, .. } => {
                let mut s = String::from("CASE");
                for arm in arms {
                    s.push_str(&format!(
                        " WHEN {} THEN {}",
                        self.predicate(&arm.when, model),
                        self.shape_expr(&arm.then, alias, prefix, model)
                    ));
                }
                s.push_str(&format!(
                    " ELSE {} END",
                    self.shape_expr(else_, alias, prefix, model)
                ));
                s
            }
        }
    }

    /// Flatten a concat chain (`a || b || c`) into its ordered operand SQL, so the whole
    /// chain lowers to one `CONCAT(…)` / `… || … || …` instead of nesting per operator.
    fn collect_concat(
        &mut self,
        expr: &'a ShapeExpr,
        alias: &str,
        prefix: &str,
        model: &'a RModel,
        parts: &mut Vec<String>,
    ) {
        match expr {
            ShapeExpr::Concat { lhs, rhs, .. } => {
                self.collect_concat(lhs, alias, prefix, model, parts);
                self.collect_concat(rhs, alias, prefix, model, parts);
            }
            other => parts.push(self.shape_expr(other, alias, prefix, model)),
        }
    }
}
