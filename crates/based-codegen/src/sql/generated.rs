//! Generated-column expressions: a `@generated(<expr>)` shape lowered to the SQL a
//! `GENERATED ALWAYS AS (…)` clause accepts, or to the re-parseable neutral DSL the
//! migration snapshot stores. Same-row-only, so bare column names and no joins.

use std::fmt::Write as _;

use super::*;

/// How a generated-column expression is rendered.
#[derive(Clone, Copy)]
pub(crate) enum Emit {
    /// Per-dialect SQL for a `GENERATED ALWAYS AS (…)` clause — columns quoted, concat
    /// through the dialect seam, enum variants as wire literals.
    Sql(Dialect),
    /// Dialect-neutral, re-parseable DSL source (bare column names, `||` concat, DSL
    /// literals) — the form the migration snapshot stores so it can be re-lowered per
    /// target dialect at migrate time.
    Neutral,
}

/// Render a **generated column** expression over the row's own columns, referenced **bare**
/// (unqualified) — the form every dialect accepts inside a `GENERATED ALWAYS AS (…)` clause.
/// Same-row-only (sema-enforced), so no joins: arithmetic lowers to `(a op b)`, concat through
/// the target's seam, and a `case` to a conditional. `Emit::Sql` produces per-dialect SQL;
/// `Emit::Neutral` produces re-parseable DSL for the snapshot. `schema`/`model` resolve
/// physical column names and enum variants; both are absent when re-lowering an already-neutral
/// (physical-name, literal-enum) expression parsed back from the snapshot.
pub(crate) fn generated_expr(
    schema: Option<&CheckedSchema>,
    model: Option<&RModel>,
    expr: &ShapeExpr,
    emit: Emit,
) -> String {
    match expr {
        ShapeExpr::Value(v) => gen_value(schema, model, v, emit),
        ShapeExpr::Arith { lhs, op, rhs, .. } => format!(
            "({} {} {})",
            generated_expr(schema, model, lhs, emit),
            dml::arith_op_sql(*op), // `+ - * /` are identical in SQL and DSL
            generated_expr(schema, model, rhs, emit),
        ),
        ShapeExpr::Concat { .. } => {
            let mut parts = Vec::new();
            gen_collect_concat(schema, model, expr, emit, &mut parts);
            match emit {
                Emit::Sql(d) => d.concat(&parts),
                Emit::Neutral => format!("({})", parts.join(" || ")),
            }
        }
        ShapeExpr::Case { arms, else_, .. } => {
            let (kw_case, kw_when, kw_then, kw_else, kw_end) = match emit {
                Emit::Sql(_) => ("CASE", "WHEN", "THEN", "ELSE", "END"),
                Emit::Neutral => ("case", "when", "then", "else", "end"),
            };
            let mut s = String::from(kw_case);
            for arm in arms {
                let _ = write!(
                    s,
                    " {kw_when} {} {kw_then} {}",
                    gen_predicate(schema, model, &arm.when, emit),
                    generated_expr(schema, model, &arm.then, emit)
                );
            }
            let _ = write!(
                s,
                " {kw_else} {} {kw_end}",
                generated_expr(schema, model, else_, emit)
            );
            s
        }
    }
}

fn gen_collect_concat(
    schema: Option<&CheckedSchema>,
    model: Option<&RModel>,
    expr: &ShapeExpr,
    emit: Emit,
    parts: &mut Vec<String>,
) {
    match expr {
        ShapeExpr::Concat { lhs, rhs, .. } => {
            gen_collect_concat(schema, model, lhs, emit, parts);
            gen_collect_concat(schema, model, rhs, emit, parts);
        }
        other => parts.push(generated_expr(schema, model, other, emit)),
    }
}

/// A leaf column name: the physical column (via `model`, else the name verbatim), rendered
/// quoted for SQL or bare for the neutral DSL form.
fn gen_col(model: Option<&RModel>, name: &str, emit: Emit) -> String {
    let col = model.map_or_else(|| name.to_string(), |m| physical_col(m, name));
    match emit {
        Emit::Sql(d) => d.quote(&col),
        Emit::Neutral => col,
    }
}

/// A generated-column leaf value: a same-row column, or a literal (SQL literal for `Emit::Sql`,
/// re-parseable DSL literal for `Emit::Neutral`).
fn gen_value(
    schema: Option<&CheckedSchema>,
    model: Option<&RModel>,
    v: &Value,
    emit: Emit,
) -> String {
    let _ = schema;
    match v {
        Value::Path(p) => gen_col(model, &p.segments[0].node, emit),
        Value::Lit(l) => match emit {
            Emit::Sql(d) => dml::render_lit(d, l),
            Emit::Neutral => neutral_lit(l),
        },
        Value::Func(f) => dml::render_func(f),
        // A parameter operand is rejected in sema before codegen.
        Value::Param(_) => String::new(),
    }
}

/// A `Literal` in re-parseable DSL source form (double-quoted string, bare number/bool/null).
fn neutral_lit(l: &Literal) -> String {
    match l {
        Literal::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Literal::Int(n) => n.to_string(),
        Literal::Decimal(d) => d.clone(),
        Literal::Bool(b) => b.to_string(),
        Literal::Null => "null".to_string(),
    }
}

/// The DSL spelling of a comparison operator (for the neutral form).
fn dsl_op(op: based_ast::Op) -> &'static str {
    use based_ast::Op;
    match op {
        Op::Eq => "=",
        Op::Ne => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "~",
        Op::In => "in",
        Op::Has => "has",
    }
}

/// Render a generated-column CASE `when` predicate, bare-columned, in SQL or neutral DSL.
fn gen_predicate(
    schema: Option<&CheckedSchema>,
    model: Option<&RModel>,
    pred: &Predicate,
    emit: Emit,
) -> String {
    let (and, or, not_) = match emit {
        Emit::Sql(_) => ("AND", "OR", "NOT"),
        Emit::Neutral => ("and", "or", "not"),
    };
    match pred {
        Predicate::And(a, b) => format!(
            "({} {and} {})",
            gen_predicate(schema, model, a, emit),
            gen_predicate(schema, model, b, emit)
        ),
        Predicate::Or(a, b) => format!(
            "({} {or} {})",
            gen_predicate(schema, model, a, emit),
            gen_predicate(schema, model, b, emit)
        ),
        Predicate::Not(inner) => {
            format!("{not_} ({})", gen_predicate(schema, model, inner, emit))
        }
        Predicate::Cmp { path, op, value } => gen_cmp(schema, model, path, *op, value, emit),
        Predicate::InList { path, values } => {
            let lhs = gen_col(model, &path.segments[0].node, emit);
            let items: Vec<String> = values
                .iter()
                .map(|v| gen_cmp_rhs(schema, model, path, v, emit))
                .collect();
            let kw = match emit {
                Emit::Sql(_) => "IN",
                Emit::Neutral => "in",
            };
            format!("{lhs} {kw} ({})", items.join(", "))
        }
        Predicate::Bare(path) => match emit {
            Emit::Sql(d) => format!(
                "{} = {}",
                gen_col(model, &path.segments[0].node, emit),
                d.bool_lit(true)
            ),
            Emit::Neutral => gen_col(model, &path.segments[0].node, emit),
        },
        // Named filters / raw are rejected in a generated column's condition during sema.
        Predicate::FilterCall { .. } | Predicate::Raw(_) => match emit {
            Emit::Sql(d) => d.bool_lit(true).to_string(),
            Emit::Neutral => "true".to_string(),
        },
    }
}

/// A single `col <op> value` comparison in a generated-column condition. A `= null` /
/// `!= null` is a null test: SQL spells it `IS [NOT] NULL`; the neutral DSL keeps `= null`
/// (which re-parses back to this same comparison).
fn gen_cmp(
    schema: Option<&CheckedSchema>,
    model: Option<&RModel>,
    path: &Path,
    op: based_ast::Op,
    value: &Value,
    emit: Emit,
) -> String {
    let lhs = gen_col(model, &path.segments[0].node, emit);
    if matches!(value, Value::Lit(Literal::Null))
        && matches!(op, based_ast::Op::Eq | based_ast::Op::Ne)
    {
        return match emit {
            Emit::Sql(_) => {
                let n = if op == based_ast::Op::Eq {
                    "IS NULL"
                } else {
                    "IS NOT NULL"
                };
                format!("{lhs} {n}")
            }
            Emit::Neutral => format!("{lhs} {} null", dsl_op(op)),
        };
    }
    let rhs = gen_cmp_rhs(schema, model, path, value, emit);
    let op_s = match emit {
        Emit::Sql(_) => dml::sql_op(op),
        Emit::Neutral => dsl_op(op),
    };
    format!("{lhs} {op_s} {rhs}")
}

/// A generated-column comparison RHS: an enum column's bare variant resolves to its wire
/// literal (SQL) / DSL literal (neutral); a same-row column to its name; a literal to its form.
fn gen_cmp_rhs(
    schema: Option<&CheckedSchema>,
    model: Option<&RModel>,
    path: &Path,
    value: &Value,
    emit: Emit,
) -> String {
    if let (Value::Path(rp), Some(schema), Some(model)) = (value, schema, model) {
        if rp.segments.len() == 1 {
            // An enum column compared to a bare variant → the variant's wire value.
            if let Some(MemberKind::Scalar {
                enum_name: Some(en),
                ..
            }) = model.member(&path.segments[0].node).map(|m| &m.kind)
            {
                if let Some(v) = schema
                    .enum_(en)
                    .and_then(|e| e.wire_of(&rp.segments[0].node))
                {
                    return match emit {
                        Emit::Sql(_) => enum_value_sql(v),
                        Emit::Neutral => neutral_enum_lit(v),
                    };
                }
            }
        }
    }
    gen_value(schema, model, value, emit)
}

/// An enum wire value as a re-parseable DSL literal: a string double-quoted, an int bare.
fn neutral_enum_lit(v: &based_sema::EnumValue) -> String {
    match v {
        based_sema::EnumValue::Str(s) => {
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        }
        based_sema::EnumValue::Int(n) => n.to_string(),
    }
}
