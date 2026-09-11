//! Precedence-driven reprinting of the recursive expression forms with minimal
//! parentheses: computed shape expressions, predicates, and assignment arithmetic.

use crate::*;

/// Reprint a computed shape expression with minimal parentheses. Precedence, tightest
/// last: concat (1) < `+`/`-` (2) < `*`/`/` (3) < a value / `case` leaf (4). A node is
/// wrapped only when it binds looser than the position it sits in (`min`).
pub(crate) fn shape_expr(e: &ShapeExpr, min: u8) -> String {
    let (prec, body) = match e {
        ShapeExpr::Value(v) => (4, value(v)),
        ShapeExpr::Case { arms, else_, .. } => {
            let mut s = String::from("case");
            for arm in arms {
                s.push_str(&format!(
                    " when {} then {}",
                    predicate(&arm.when, 0),
                    shape_expr(&arm.then, 0)
                ));
            }
            s.push_str(&format!(" else {} end", shape_expr(else_, 0)));
            (4, s)
        }
        ShapeExpr::Arith { lhs, op, rhs, .. } => {
            let prec = match op {
                ArithOp::Add | ArithOp::Sub => 2,
                ArithOp::Mul | ArithOp::Div => 3,
            };
            (
                prec,
                format!(
                    "{} {} {}",
                    shape_expr(lhs, prec),
                    arith_op(*op),
                    shape_expr(rhs, prec + 1)
                ),
            )
        }
        ShapeExpr::Concat { lhs, rhs, .. } => (
            1,
            format!("{} || {}", shape_expr(lhs, 1), shape_expr(rhs, 2)),
        ),
    };
    if prec < min {
        format!("({body})")
    } else {
        body
    }
}

/// Precedence of the top operator: `or` < `and` < everything atomic. `min` is the
/// precedence the enclosing context requires — a lower-precedence child is wrapped.
pub(crate) fn predicate(p: &Predicate, min: u8) -> String {
    let (prec, body) = match p {
        Predicate::Or(a, b) => (1, format!("{} or {}", predicate(a, 1), predicate(b, 2))),
        Predicate::And(a, b) => (2, format!("{} and {}", predicate(a, 2), predicate(b, 3))),
        Predicate::Not(a) => (3, format!("not {}", predicate(a, 4))),
        Predicate::Cmp {
            path: pa,
            op,
            value: v,
        } => (4, format!("{} {} {}", path(pa), op_str(*op), value(v))),
        Predicate::InList { path: pa, values } => (
            4,
            format!(
                "{} in ({})",
                path(pa),
                values.iter().map(value).collect::<Vec<_>>().join(", ")
            ),
        ),
        Predicate::Bare(pa) => (4, path(pa)),
        Predicate::FilterCall { name, args } => (
            4,
            format!(
                "{}({})",
                name.node,
                args.iter().map(value).collect::<Vec<_>>().join(", ")
            ),
        ),
        Predicate::Raw(r) => (4, raw_sql(r)),
    };
    if prec < min {
        format!("({body})")
    } else {
        body
    }
}

pub(crate) fn op_str(op: Op) -> &'static str {
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

/// An assignment RHS: a plain value, or an arithmetic expression with minimal
/// parentheses (only where precedence/associativity require them).
pub(crate) fn assign_rhs(r: &AssignRhs) -> String {
    arith(r, 0)
}

fn arith(r: &AssignRhs, min: u8) -> String {
    let (prec, body) = match r {
        AssignRhs::Value(v) => (3, value(v)),
        AssignRhs::Arith { lhs, op, rhs, .. } => {
            let prec = match op {
                ArithOp::Add | ArithOp::Sub => 1,
                ArithOp::Mul | ArithOp::Div => 2,
            };
            (
                prec,
                format!(
                    "{} {} {}",
                    arith(lhs, prec),
                    arith_op(*op),
                    arith(rhs, prec + 1)
                ),
            )
        }
    };
    if prec < min {
        format!("({body})")
    } else {
        body
    }
}

fn arith_op(op: ArithOp) -> &'static str {
    match op {
        ArithOp::Add => "+",
        ArithOp::Sub => "-",
        ArithOp::Mul => "*",
        ArithOp::Div => "/",
    }
}
