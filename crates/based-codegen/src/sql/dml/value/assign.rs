//! Lower an update/upsert assignment RHS (enum variant, arithmetic, composite-FK) to SQL.

use super::super::*;

impl<'a> Select<'a> {
    /// Lower an assignment RHS: a plain value (an enum column's variant → its wire
    /// literal, else the ordinary value lowering), or an arithmetic expression
    /// (`total + $n`) lowered to a real SQL `(a op b)` — a column operand reads the
    /// row's pre-write value, computed in the database, never read-modify-write.
    pub(crate) fn assign_rhs(
        &mut self,
        rhs: &AssignRhs,
        model: &RModel,
        col_field: &str,
    ) -> String {
        match rhs {
            AssignRhs::Value(v) => self
                .enum_assign_lit(model, col_field, v)
                .unwrap_or_else(|| self.value(v, model)),
            AssignRhs::Arith { lhs, op, rhs, .. } => format!(
                "({} {} {})",
                self.assign_rhs(lhs, model, col_field),
                arith_op_sql(*op),
                self.assign_rhs(rhs, model, col_field)
            ),
        }
    }

    /// The value SQL for one key part of a composite-FK assign (`enrollment = $rhs`). A tx
    /// binding (`$e`) pulls the part from the bound create's matching key-part assign; a
    /// plain structured-id param (`$enrollment`) binds a per-part placeholder the runtime
    /// splits from the JSON object.
    pub(crate) fn fk_assign_part(&self, rhs: &AssignRhs, part_field: &str) -> String {
        if let AssignRhs::Value(Value::Param(pr)) = rhs {
            if self.bindings.contains_key(pr.name.node.as_str()) {
                return self.binding_field_value(&pr.name.node, part_field);
            }
            return format!(":{}__{}", pr.name.node, part_field);
        }
        "NULL".to_string()
    }
}
