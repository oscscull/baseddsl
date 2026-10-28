//! Map a DSL comparison and arithmetic operator to SQL.

use super::*;

/// `lhs op rhs` for one comparison. `in` takes a value list; `has` is JSON-array containment
/// (Postgres `arr @> value`, MySQL-family `value MEMBER OF(arr)`); `~ all` / `~ any` match a
/// pattern list.
pub(crate) fn compare_sql(dialect: Dialect, lhs: &str, op: Op, rhs: &str) -> String {
    let sym = match op {
        Op::Eq => "=",
        Op::Ne => "<>",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "LIKE",
        Op::LikeAll => return like_list(dialect, lhs, Quantifier::All, rhs),
        Op::LikeAny => return like_list(dialect, lhs, Quantifier::Any, rhs),
        Op::In => return format!("{lhs} IN ({rhs})"),
        Op::Has => {
            return match dialect {
                Dialect::Postgres => format!("{lhs} @> {rhs}"),
                _ => format!("{rhs} MEMBER OF({lhs})"),
            }
        }
    };
    format!("{lhs} {sym} {rhs}")
}

pub(crate) fn arith_op_sql(op: ArithOp) -> &'static str {
    match op {
        ArithOp::Add => "+",
        ArithOp::Sub => "-",
        ArithOp::Mul => "*",
        ArithOp::Div => "/",
    }
}
