use super::*;

impl<'a> Parser<'a> {
    /// An assignment RHS: a plain `value`, or a scalar arithmetic expression over the
    /// target model's numeric columns + params (`total = total + $n`). `+`/`-` are the
    /// lowest-precedence, left-associative level.
    pub(super) fn assign_rhs(&mut self) -> PResult<AssignRhs> {
        let start = self.here().start;
        let mut lhs = self.arith_term()?;
        loop {
            let op = if self.at(Tok::Plus) {
                ArithOp::Add
            } else if self.at(Tok::Minus) {
                ArithOp::Sub
            } else {
                break;
            };
            self.bump();
            let rhs = self.arith_term()?;
            lhs = self.mk_arith(lhs, op, rhs, start);
        }
        Ok(lhs)
    }

    /// `*`/`/` — the tighter-binding, left-associative arithmetic level.
    fn arith_term(&mut self) -> PResult<AssignRhs> {
        let start = self.here().start;
        let mut lhs = self.arith_factor()?;
        loop {
            let op = if self.at(Tok::Star) {
                ArithOp::Mul
            } else if self.at(Tok::Slash) {
                ArithOp::Div
            } else {
                break;
            };
            self.bump();
            let rhs = self.arith_factor()?;
            lhs = self.mk_arith(lhs, op, rhs, start);
        }
        Ok(lhs)
    }

    /// A leaf of an arithmetic RHS: a `value`, or a parenthesized subexpression.
    fn arith_factor(&mut self) -> PResult<AssignRhs> {
        if self.at(Tok::LParen) {
            self.bump();
            let inner = self.assign_rhs()?;
            self.expect(Tok::RParen, "`)`")?;
            Ok(inner)
        } else {
            Ok(AssignRhs::Value(self.value()?))
        }
    }

    fn mk_arith(&self, lhs: AssignRhs, op: ArithOp, rhs: AssignRhs, start: u32) -> AssignRhs {
        AssignRhs::Arith {
            lhs: Box::new(lhs),
            op,
            rhs: Box::new(rhs),
            span: Span {
                file: self.file,
                start,
                end: self.prev_end(),
            },
        }
    }
}
