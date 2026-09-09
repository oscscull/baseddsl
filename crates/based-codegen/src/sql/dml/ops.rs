//! Map a DSL operator to its SQL keyword/symbol.

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
