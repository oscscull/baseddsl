use super::*;

/// A generated column (`name = <expr>`) as a real scalar member. Its `ty`/`optional` start
/// as placeholders — [`resolve_generated`] infers them once every member exists.
pub(crate) fn generated_member(g: &GeneratedField) -> RMember {
    RMember {
        name: g.name.node.clone(),
        span: g.name.span,
        kind: MemberKind::Scalar {
            ty: Primitive::Text,
            optional: false,
            many: false,
            column: g.name.node.clone(),
            unique: false,
            default: None,
            enum_name: None,
            raw_type: None,
            generated: Some(g.expr.clone()),
        },
        was: None,
        sort: Vec::new(),
    }
}

/// A base (non-generated) scalar column's type facts, read while typing a generated column.
#[derive(Clone, Copy)]
struct ColInfo {
    ty: Primitive,
    optional: bool,
}

/// Coarse value families a generated-column operand can carry.
#[derive(Clone, Copy, PartialEq)]
enum GFamily {
    Numeric,
    Textual,
    Bool,
    /// A type outside the checked families (temporal / json / bytes) or an unknown leaf —
    /// never triggers a family mismatch on its own.
    Other,
}

fn prim_family(p: Primitive) -> GFamily {
    match p {
        Primitive::Int | Primitive::Float | Primitive::Decimal { .. } | Primitive::Serial => {
            GFamily::Numeric
        }
        Primitive::Text | Primitive::Uuid | Primitive::Ulid | Primitive::Id => GFamily::Textual,
        Primitive::Bool => GFamily::Bool,
        _ => GFamily::Other,
    }
}

/// Validate every generated column's expression (same-row-only, operand families) and infer
/// its type + nullability from the row's own columns. A generated column lowers to a stored
/// `GENERATED ALWAYS AS (…) STORED` column, so its operands are the row's own non-generated
/// columns — a relation reach, an aggregate, or a parameter has no meaning and is rejected.
pub(crate) fn resolve_generated(members: &mut [RMember], sink: &mut Sink) {
    let cols: HashMap<String, ColInfo> = members
        .iter()
        .filter_map(|m| match &m.kind {
            MemberKind::Scalar {
                ty,
                optional,
                generated: None,
                ..
            } => Some((
                m.name.clone(),
                ColInfo {
                    ty: *ty,
                    optional: *optional,
                },
            )),
            _ => None,
        })
        .collect();
    let gen_names: std::collections::HashSet<String> = members
        .iter()
        .filter(|m| m.kind.is_generated())
        .map(|m| m.name.clone())
        .collect();

    let mut inferred: Vec<(usize, Primitive, bool)> = Vec::new();
    for (i, m) in members.iter().enumerate() {
        let Some(expr) = m.kind.generated() else {
            continue;
        };
        check_generated_expr(expr, &cols, &gen_names, sink);
        let (prim, opt) = infer_generated(expr, &cols);
        inferred.push((i, prim.unwrap_or(Primitive::Text), opt));
    }
    for (i, ty, opt) in inferred {
        if let MemberKind::Scalar {
            ty: t, optional, ..
        } = &mut members[i].kind
        {
            *t = ty;
            *optional = opt;
        }
    }
}

/// Type-check a generated-column expression, emitting the E034x diagnostics; returns its
/// coarse family (`None` for a null literal / unmodelled leaf, which never mismatches).
fn check_generated_expr(
    expr: &ShapeExpr,
    cols: &HashMap<String, ColInfo>,
    gen_names: &std::collections::HashSet<String>,
    sink: &mut Sink,
) -> Option<GFamily> {
    match expr {
        ShapeExpr::Value(v) => gen_operand_family(v, cols, gen_names, sink),
        ShapeExpr::Arith { lhs, rhs, span, .. } => {
            gen_require(
                lhs,
                GFamily::Numeric,
                code::GEN_ARITH_OPERAND,
                *span,
                cols,
                gen_names,
                sink,
            );
            gen_require(
                rhs,
                GFamily::Numeric,
                code::GEN_ARITH_OPERAND,
                *span,
                cols,
                gen_names,
                sink,
            );
            Some(GFamily::Numeric)
        }
        ShapeExpr::Concat { lhs, rhs, span } => {
            gen_require(
                lhs,
                GFamily::Textual,
                code::GEN_CONCAT_OPERAND,
                *span,
                cols,
                gen_names,
                sink,
            );
            gen_require(
                rhs,
                GFamily::Textual,
                code::GEN_CONCAT_OPERAND,
                *span,
                cols,
                gen_names,
                sink,
            );
            Some(GFamily::Textual)
        }
        ShapeExpr::Case { arms, else_, span } => {
            for arm in arms {
                gen_check_when(&arm.when, cols, gen_names, sink);
            }
            let mut result: Option<GFamily> = None;
            let mut mismatch = false;
            for branch in arms
                .iter()
                .map(|a| &a.then)
                .chain(std::iter::once(&**else_))
            {
                if let Some(f) = check_generated_expr(branch, cols, gen_names, sink) {
                    match result {
                        None => result = Some(f),
                        Some(r) if r == f => {}
                        Some(_) => mismatch = true,
                    }
                }
            }
            if mismatch {
                sink.error_note(
                    code::GEN_CASE_MISMATCH,
                    *span,
                    "a `case`'s branches have different types",
                    "every `then`/`else` branch must produce the same kind of value",
                );
            }
            result
        }
    }
}

/// Require a generated operand to be of `want`'s family, emitting `code` otherwise.
fn gen_require(
    e: &ShapeExpr,
    want: GFamily,
    code: &'static str,
    span: Span,
    cols: &HashMap<String, ColInfo>,
    gen_names: &std::collections::HashSet<String>,
    sink: &mut Sink,
) {
    let Some(fam) = check_generated_expr(e, cols, gen_names, sink) else {
        return;
    };
    if fam != want && fam != GFamily::Other {
        let msg = if want == GFamily::Numeric {
            "an arithmetic operand must be numeric (int/float/decimal)"
        } else {
            "a `||` concatenation operand must be text"
        };
        sink.error(code, span, msg.to_string());
    }
}

/// Family of a single generated-column leaf operand, name-checked against the row's columns.
fn gen_operand_family(
    v: &Value,
    cols: &HashMap<String, ColInfo>,
    gen_names: &std::collections::HashSet<String>,
    sink: &mut Sink,
) -> Option<GFamily> {
    match v {
        Value::Param(pr) => {
            sink.error_note(
                code::GEN_PARAM,
                pr.name.span,
                format!("`${}` — a generated column has no parameters", pr.name.node),
                "generated-column operands are the row's own columns and literals",
            );
            None
        }
        Value::Lit(l) => match l {
            Literal::Int(_) | Literal::Decimal(_) => Some(GFamily::Numeric),
            Literal::Str(_) => Some(GFamily::Textual),
            Literal::Bool(_) => Some(GFamily::Bool),
            Literal::Null => None,
        },
        Value::Path(p) => gen_col_ref(p, cols, gen_names, sink).map(prim_family),
        // A function's return type is not modelled — treated as unknown (never a mismatch).
        Value::Func(_) => None,
    }
}

/// Resolve a generated-column operand path to its column type, enforcing same-row-only: the
/// path must be a single segment (no relation reach) naming a base column of this model, not
/// another generated column.
fn gen_col_ref(
    p: &Path,
    cols: &HashMap<String, ColInfo>,
    gen_names: &std::collections::HashSet<String>,
    sink: &mut Sink,
) -> Option<Primitive> {
    let seg = p.segments.first()?;
    if p.segments.len() > 1 {
        sink.error_note(
            code::GEN_REACH,
            seg.span,
            format!("`{}` reaches through a relation", render_path(p)),
            "a stored generated column sees only its own row's columns",
        );
        return None;
    }
    if gen_names.contains(&seg.node) {
        sink.error_note(
            code::GEN_ON_GENERATED,
            seg.span,
            format!("`{}` is another generated column", seg.node),
            "a stored generated column cannot depend on another generated column",
        );
        return None;
    }
    let Some(ci) = cols.get(&seg.node) else {
        sink.error(
            code::GEN_UNKNOWN_COL,
            seg.span,
            format!("`{}` is not a column of this model", seg.node),
        );
        return None;
    };
    Some(ci.ty)
}

/// A CASE arm's `when` predicate: reject parameters and relation reaches, and name-check the
/// left operand of each comparison against the row's own columns.
fn gen_check_when(
    pred: &Predicate,
    cols: &HashMap<String, ColInfo>,
    gen_names: &std::collections::HashSet<String>,
    sink: &mut Sink,
) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            gen_check_when(a, cols, gen_names, sink);
            gen_check_when(b, cols, gen_names, sink);
        }
        Predicate::Not(inner) => gen_check_when(inner, cols, gen_names, sink),
        Predicate::Cmp { path, value, .. } => {
            gen_col_ref(path, cols, gen_names, sink);
            gen_when_value(value, sink);
        }
        Predicate::InList { path, values } => {
            gen_col_ref(path, cols, gen_names, sink);
            for v in values {
                gen_when_value(v, sink);
            }
        }
        Predicate::Bare(path) => {
            gen_col_ref(path, cols, gen_names, sink);
        }
        // A named filter / raw term could pull in another table; neither belongs in a stored
        // generated column's condition.
        Predicate::FilterCall { name, .. } => sink.error_note(
            code::GEN_REACH,
            name.span,
            format!("`{}` is a named filter", name.node),
            "a generated column's `case` condition uses only the row's own columns",
        ),
        Predicate::Raw(_) => {}
    }
}

/// A comparison RHS inside a generated CASE: a `$…` is rejected (no parameters); a dotted
/// path is a relation reach. A bare identifier may be an enum variant, so it is left alone.
fn gen_when_value(v: &Value, sink: &mut Sink) {
    match v {
        Value::Param(pr) => sink.error_note(
            code::GEN_PARAM,
            pr.name.span,
            format!("`${}` — a generated column has no parameters", pr.name.node),
            "generated-column operands are the row's own columns and literals",
        ),
        Value::Path(p) if p.segments.len() > 1 => sink.error_note(
            code::GEN_REACH,
            p.segments[0].span,
            format!("`{}` reaches through a relation", render_path(p)),
            "a stored generated column sees only its own row's columns",
        ),
        _ => {}
    }
}

fn render_path(p: &Path) -> String {
    p.segments
        .iter()
        .map(|s| s.node.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// Infer a generated column's `(type, nullable)` from its expression: a column operand
/// carries its own type/nullability, arithmetic promotes over the numeric family
/// (decimal > float > int), concat is text, a CASE unifies its branches and is nullable if
/// any branch is nullable. `None` = an unmodelled/`null` result (the caller falls back to a
/// nullable text column).
fn infer_generated(expr: &ShapeExpr, cols: &HashMap<String, ColInfo>) -> (Option<Primitive>, bool) {
    match expr {
        ShapeExpr::Value(v) => match v {
            Value::Path(p) if p.segments.len() == 1 => match cols.get(&p.segments[0].node) {
                Some(ci) => (Some(ci.ty), ci.optional),
                None => (None, true),
            },
            Value::Path(_) => (None, true),
            Value::Lit(Literal::Int(_)) => (Some(Primitive::Int), false),
            Value::Lit(Literal::Decimal(_)) => (
                Some(Primitive::Decimal {
                    precision: 38,
                    scale: 9,
                }),
                false,
            ),
            Value::Lit(Literal::Str(_)) => (Some(Primitive::Text), false),
            Value::Lit(Literal::Bool(_)) => (Some(Primitive::Bool), false),
            Value::Lit(Literal::Null) => (None, true),
            Value::Param(_) | Value::Func(_) => (None, true),
        },
        ShapeExpr::Arith { lhs, rhs, .. } => {
            let (a, ao) = infer_generated(lhs, cols);
            let (b, bo) = infer_generated(rhs, cols);
            (promote_numeric(a, b), ao || bo)
        }
        ShapeExpr::Concat { lhs, rhs, .. } => {
            let (_, ao) = infer_generated(lhs, cols);
            let (_, bo) = infer_generated(rhs, cols);
            (Some(Primitive::Text), ao || bo)
        }
        ShapeExpr::Case { arms, else_, .. } => {
            let mut prim = None;
            let mut optional = false;
            for branch in arms
                .iter()
                .map(|a| &a.then)
                .chain(std::iter::once(&**else_))
            {
                let (p, o) = infer_generated(branch, cols);
                optional |= o;
                prim = match (prim, p) {
                    (None, p) => p,
                    (Some(x), Some(y)) if x == y => Some(x),
                    (Some(x), Some(y)) => promote_numeric(Some(x), Some(y)),
                    (acc, None) => acc,
                };
            }
            (prim, optional)
        }
    }
}

fn promote_numeric(a: Option<Primitive>, b: Option<Primitive>) -> Option<Primitive> {
    let (a, b) = (a?, b?);
    let rank = |p: Primitive| match p {
        Primitive::Decimal { .. } => Some(3),
        Primitive::Float => Some(2),
        Primitive::Int | Primitive::Serial => Some(1),
        _ => None,
    };
    let widen = |p: Primitive| match p {
        Primitive::Serial => Primitive::Int,
        Primitive::Decimal { .. } => Primitive::Decimal {
            precision: 38,
            scale: 9,
        },
        other => other,
    };
    match (rank(a), rank(b)) {
        (Some(ra), Some(rb)) => Some(if ra >= rb { widen(a) } else { widen(b) }),
        _ => None,
    }
}
