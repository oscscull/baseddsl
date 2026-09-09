//! Path type/primitive inference and computed-expression result typing.

use super::*;

/// The primitive a dotted sort path terminates in, walked against the schema: a scalar
/// is its own primitive, a relation terminal is the FK it sorts by (a uuid). An
/// unresolved path (sema already flagged) falls back to `Text` so lowering terminates.
pub(crate) fn path_primitive(schema: &CheckedSchema, root: &RModel, path: &Path) -> Primitive {
    let mut cur = root;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind) {
            Some(MemberKind::Scalar { ty, .. }) => return *ty,
            Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
                if last {
                    // The FK's primitive mirrors the target model's primary-key type
                    // (a serial target → an `int` FK, a uuid target → uuid).
                    return schema
                        .model(target)
                        .and_then(RModel::pk_member)
                        .and_then(|m| match &m.kind {
                            MemberKind::Scalar { ty, .. } => Some(*ty),
                            _ => None,
                        })
                        .unwrap_or(Primitive::Uuid);
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return Primitive::Text,
                }
            }
            None => return Primitive::Text,
        }
    }
    Primitive::Text
}

/// The primitive a dotted path lands on, walking relations, or `None` when it doesn't
/// terminate on a scalar. Shared by the computed-field type inference below.
pub(crate) fn path_scalar_primitive(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    path: &Path,
) -> Option<Primitive> {
    let mut cur = model?;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind)? {
            MemberKind::Scalar { ty, .. } if last => return Some(*ty),
            MemberKind::Scalar { .. } => return None,
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                if last {
                    return None;
                }
                cur = schema.model(target)?;
            }
        }
    }
    None
}

/// The Rust/OpenAPI result type of a computed shape-field expression: the terminating
/// primitive (or `None` for an unknown/opaque leaf → `Json`) and whether it is nullable.
/// Arithmetic promotes over the numeric family (decimal > float > int); concat is text; a
/// CASE unifies its branches (numeric branches promote) and is nullable if any branch is a
/// `null` literal. Both the client and the OpenAPI emitters read the field's type from here
/// so the two can't drift.
pub(crate) fn computed_result(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    expr: &ShapeExpr,
) -> (Option<Primitive>, bool) {
    match expr {
        ShapeExpr::Value(v) => match v {
            Value::Path(p) => (path_scalar_primitive(schema, model, p), false),
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
            // Params are rejected in a shape (sema E0323); a function's type is unmodelled.
            Value::Param(_) | Value::Func(_) => (None, false),
        },
        ShapeExpr::Arith { lhs, rhs, .. } => {
            let (a, ao) = computed_result(schema, model, lhs);
            let (b, bo) = computed_result(schema, model, rhs);
            (promote_numeric(a, b), ao || bo)
        }
        ShapeExpr::Concat { .. } => (Some(Primitive::Text), false),
        ShapeExpr::Case { arms, else_, .. } => {
            let mut prim = None;
            let mut optional = false;
            for branch in arms
                .iter()
                .map(|a| &a.then)
                .chain(std::iter::once(&**else_))
            {
                let (p, o) = computed_result(schema, model, branch);
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

/// Promote two numeric operands to their common type: decimal beats float beats int; an
/// unknown operand (`None`) or a non-numeric leaves the result unknown.
fn promote_numeric(a: Option<Primitive>, b: Option<Primitive>) -> Option<Primitive> {
    let (a, b) = (a?, b?);
    let rank = |p: Primitive| match p {
        Primitive::Decimal { .. } => Some(3),
        Primitive::Float => Some(2),
        Primitive::Int | Primitive::Serial => Some(1),
        _ => None,
    };
    match (rank(a), rank(b)) {
        (Some(ra), Some(rb)) => Some(if ra >= rb { widen(a) } else { widen(b) }),
        _ => None,
    }
}

/// The canonical primitive for a promoted numeric result (a bare `decimal(38, 9)`, or the
/// operand's own type).
fn widen(p: Primitive) -> Primitive {
    match p {
        Primitive::Serial => Primitive::Int,
        Primitive::Decimal { .. } => Primitive::Decimal {
            precision: 38,
            scale: 9,
        },
        other => other,
    }
}

/// Whether a dotted sort path terminates in a nullable column: a scalar's `optional`, a
/// forward relation's nullable FK. A to-many/inverse terminal or an unresolved path is
/// treated as non-nullable (the keyset comparison stays in its plain, non-NULL form).
pub(crate) fn path_nullable(schema: &CheckedSchema, root: &RModel, path: &Path) -> bool {
    let mut cur = root;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind) {
            Some(MemberKind::Scalar { optional, .. }) => return *optional,
            Some(MemberKind::Forward {
                target, optional, ..
            }) => {
                if last {
                    return *optional;
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return false,
                }
            }
            Some(MemberKind::Inverse { target, .. }) => match schema.model(target) {
                Some(m) if !last => cur = m,
                _ => return false,
            },
            None => return false,
        }
    }
    false
}
