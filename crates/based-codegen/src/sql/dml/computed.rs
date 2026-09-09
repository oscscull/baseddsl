//! Computed-expression result typing and the numeric promotion lattice, plus the per-dialect
//! numeric cast targets aggregates decode through.

use super::*;

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

/// The `CAST(… AS <int>)` target that coerces a widened `SUM(int)` back to an integer, or
/// `None` where the dialect keeps it integral (SQLite). MariaDB/Postgres widen `SUM` of a
/// `BIGINT` to decimal/numeric, which would decode as a string; the cast keeps it a number.
pub(crate) fn int_cast_type(dialect: Dialect) -> Option<&'static str> {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => Some("SIGNED"),
        Dialect::Postgres => Some("BIGINT"),
        Dialect::Sqlite => None,
    }
}

/// The dialect's double type, the `CAST` target that makes `AVG` decode as a float number
/// on every dialect (Postgres `AVG` of an int/numeric is otherwise a numeric string).
pub(crate) fn double_cast_type(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => "DOUBLE",
        Dialect::Postgres => "DOUBLE PRECISION",
        Dialect::Sqlite => "REAL",
    }
}
