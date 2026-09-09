use super::*;

impl<'a> Parser<'a> {
    /// A per-row scalar expression: string concatenation is the lowest-precedence binary
    /// level, below arithmetic (concat yields text, arithmetic numbers — mixing them is a
    /// type error sema catches, so the relative binding rarely matters; concat sits lowest
    /// so `a + b || c` groups the arithmetic first).
    pub(super) fn shape_expr(&mut self) -> PResult<ShapeExpr> {
        let start = self.here().start;
        let mut lhs = self.shape_add()?;
        while self.at(Tok::PipePipe) {
            self.bump();
            let rhs = self.shape_add()?;
            lhs = ShapeExpr::Concat {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        Ok(lhs)
    }

    /// `+`/`-` — the lower-precedence arithmetic level.
    fn shape_add(&mut self) -> PResult<ShapeExpr> {
        let start = self.here().start;
        let mut lhs = self.shape_mul()?;
        loop {
            let op = if self.at(Tok::Plus) {
                ArithOp::Add
            } else if self.at(Tok::Minus) {
                ArithOp::Sub
            } else {
                break;
            };
            self.bump();
            let rhs = self.shape_mul()?;
            lhs = ShapeExpr::Arith {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        Ok(lhs)
    }

    /// `*`/`/` — the tighter-binding arithmetic level.
    fn shape_mul(&mut self) -> PResult<ShapeExpr> {
        let start = self.here().start;
        let mut lhs = self.shape_primary()?;
        loop {
            let op = if self.at(Tok::Star) {
                ArithOp::Mul
            } else if self.at(Tok::Slash) {
                ArithOp::Div
            } else {
                break;
            };
            self.bump();
            let rhs = self.shape_primary()?;
            lhs = ShapeExpr::Arith {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        Ok(lhs)
    }

    /// A leaf: a parenthesized subexpression, a `case … end`, or a value (column reach,
    /// literal, or enum variant).
    fn shape_primary(&mut self) -> PResult<ShapeExpr> {
        if self.at(Tok::LParen) {
            self.bump();
            let inner = self.shape_expr()?;
            self.expect(Tok::RParen, "`)`")?;
            return Ok(inner);
        }
        if self.at_kw("case") {
            return self.shape_case();
        }
        Ok(ShapeExpr::Value(self.value()?))
    }

    /// `case when <pred> then <expr> [when …] else <expr> end` — a conditional. One or more
    /// `when`/`then` arms then a mandatory `else` (a total expression: every row takes a
    /// branch).
    fn shape_case(&mut self) -> PResult<ShapeExpr> {
        let start = self.bump().unwrap().start; // `case`
        let mut arms = Vec::new();
        while self.eat_kw("when") {
            let when = self.predicate()?;
            if !self.eat_kw("then") {
                self.err("expected `then` after a `case` condition");
                return Err(());
            }
            let then = self.shape_expr()?;
            arms.push(CaseArm { when, then });
        }
        if arms.is_empty() {
            self.err("a `case` needs at least one `when … then …`");
            return Err(());
        }
        if !self.eat_kw("else") {
            self.err("a `case` needs an `else` branch");
            return Err(());
        }
        let else_ = self.shape_expr()?;
        if !self.eat_kw("end") {
            self.err("expected `end` to close the `case`");
            return Err(());
        }
        Ok(ShapeExpr::Case {
            arms,
            else_: Box::new(else_),
            span: self.span_from(start),
        })
    }
}
