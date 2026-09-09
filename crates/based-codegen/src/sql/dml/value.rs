//! Rendering a value/literal/function/assignment RHS to SQL text.

use super::*;

pub(crate) fn sql_op(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "<>",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "LIKE",
        Op::In => "IN",
        Op::Has => "MEMBER OF", // JSON array containment (MariaDB `x MEMBER OF(json)`)
    }
}

pub(crate) fn arith_op_sql(op: ArithOp) -> &'static str {
    match op {
        ArithOp::Add => "+",
        ArithOp::Sub => "-",
        ArithOp::Mul => "*",
        ArithOp::Div => "/",
    }
}

/// A bare single-segment variant value rendered as its wire value literal — a quoted
/// string for a string enum, a bare integer for an int enum — or `None` when `value` is
/// not a bare identifier (a `$param` binds normally; anything else falls through to
/// ordinary value lowering) or names no variant of `en`.
fn variant_lit(dialect: Dialect, en: &REnum, value: &Value) -> Option<String> {
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

impl<'a> Select<'a> {
    /// The enum a dotted path terminates on, when the terminal column is enum-typed
    /// (read-only, no join materialized). Lets the caller render a variant RHS as its
    /// wire value.
    fn terminal_enum(&self, path: &Path, model: &RModel) -> Option<&REnum> {
        let mut cur = model;
        let n = path.segments.len();
        for (i, seg) in path.segments.iter().enumerate() {
            let mem = cur.member(&seg.node)?;
            let last = i + 1 == n;
            match &mem.kind {
                MemberKind::Scalar {
                    enum_name: Some(name),
                    ..
                } if last => return self.schema.enum_(name),
                MemberKind::Scalar { .. } => return None,
                MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                    if last {
                        return None;
                    }
                    cur = self.schema.model(target)?;
                }
            }
        }
        None
    }

    /// If `path` names an enum column and `value` is a bare single-segment variant,
    /// its wire value literal (`'paid'` or `2`); else `None` to fall back to value lowering.
    pub(crate) fn enum_variant_lit(
        &self,
        model: &RModel,
        path: &Path,
        value: &Value,
    ) -> Option<String> {
        let en = self.terminal_enum(path, model)?;
        variant_lit(self.dialect, en, value)
    }

    /// If assigning enum column `col_field` a bare single-segment variant, its wire value
    /// literal; else `None`.
    pub(crate) fn enum_assign_lit(
        &self,
        model: &RModel,
        col_field: &str,
        value: &Value,
    ) -> Option<String> {
        match model.member(col_field).map(|m| &m.kind) {
            Some(MemberKind::Scalar {
                enum_name: Some(name),
                ..
            }) => {
                let en = self.schema.enum_(name)?;
                variant_lit(self.dialect, en, value)
            }
            _ => None,
        }
    }

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
    fn binding_field_value(&self, name: &str, field: &str) -> String {
        let column = self
            .schema
            .model(self.bindings[name].model)
            .map_or_else(|| field.to_string(), |m| physical_col(m, field));
        format!(":{}", bref_name(name, &column))
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
