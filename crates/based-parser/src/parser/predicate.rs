use super::*;

impl<'a> Parser<'a> {
    pub(super) fn predicate(&mut self) -> PResult<Predicate> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> PResult<Predicate> {
        let mut lhs = self.and_expr()?;
        while self.eat_kw("or") {
            let rhs = self.and_expr()?;
            lhs = Predicate::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> PResult<Predicate> {
        let mut lhs = self.not_expr()?;
        while self.eat_kw("and") {
            let rhs = self.not_expr()?;
            lhs = Predicate::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn not_expr(&mut self) -> PResult<Predicate> {
        if self.eat_kw("not") {
            Ok(Predicate::Not(Box::new(self.atom()?)))
        } else {
            self.atom()
        }
    }

    fn atom(&mut self) -> PResult<Predicate> {
        if self.eat(Tok::LParen) {
            let p = self.predicate()?;
            self.expect(Tok::RParen, "`)`")?;
            return Ok(p);
        }
        if self.is_raw_start() {
            return Ok(Predicate::Raw(self.raw_sql()?));
        }
        // A leading lower ident is a path, a comparison, or a filter call.
        let first = self.lower_ident("a column, filter, or predicate")?;
        if self.at(Tok::LParen) {
            // filter call: `name(args)`
            self.bump();
            let mut args = Vec::new();
            if !self.at(Tok::RParen) {
                loop {
                    args.push(self.value()?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
            }
            self.expect(Tok::RParen, "`)`")?;
            return Ok(Predicate::FilterCall { name: first, args });
        }
        let path = self.path_from(first);
        if let Some(any) = self.relation_any(&path)? {
            return Ok(any);
        }
        if self.at_op() {
            let op = self.op()?;
            // `in (` opens a value list; `in` with a bare value stays a plain Cmp.
            if op == Op::In && self.eat(Tok::LParen) {
                let mut values = Vec::new();
                loop {
                    values.push(self.value()?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RParen, "`)`")?;
                return Ok(Predicate::InList { path, values });
            }
            let value = self.value()?;
            Ok(Predicate::Cmp { path, op, value })
        } else {
            // bare filter reference or a bool column, both `Bare(path)`
            Ok(Predicate::Bare(path))
        }
    }

    /// `rel.any(pred)` / `rel.any()`: a path whose last segment is `any` followed by `(`.
    fn relation_any(&mut self, path: &Path) -> PResult<Option<Predicate>> {
        let is_any = path.segments.len() > 1
            && path.segments.last().is_some_and(|s| s.node == "any")
            && self.at(Tok::LParen);
        if !is_any {
            return Ok(None);
        }
        self.bump();
        // `by <fk column>` leads only when followed by `,` or `)` (a column named `by`
        // compared to something stays a predicate).
        let by = if self.at_kw("by")
            && self.tok_at(1) == Some(Tok::LowerIdent)
            && matches!(self.tok_at(2), Some(Tok::Comma | Tok::RParen))
        {
            self.bump();
            let col = self.lower_ident("a foreign-key column")?;
            self.eat(Tok::Comma);
            Some(col)
        } else {
            None
        };
        let pred = if self.at(Tok::RParen) {
            None
        } else {
            Some(Box::new(self.predicate()?))
        };
        self.expect(Tok::RParen, "`)`")?;
        let mut rel = path.clone();
        rel.segments.pop();
        Ok(Some(Predicate::Any {
            path: rel,
            by,
            pred,
        }))
    }

    fn at_op(&self) -> bool {
        matches!(
            self.peek().map(|l| l.tok),
            Some(Tok::Eq | Tok::Ne | Tok::Gt | Tok::Lt | Tok::Ge | Tok::Le | Tok::Tilde)
        ) || matches!(self.ident_at(0), Some("in" | "has"))
    }

    pub(super) fn op(&mut self) -> PResult<Op> {
        let op = match self.peek().map(|l| l.tok) {
            Some(Tok::Eq) => Op::Eq,
            Some(Tok::Ne) => Op::Ne,
            Some(Tok::Gt) => Op::Gt,
            Some(Tok::Lt) => Op::Lt,
            Some(Tok::Ge) => Op::Ge,
            Some(Tok::Le) => Op::Le,
            Some(Tok::Tilde) => Op::Like,
            Some(Tok::LowerIdent) => match self.ident_at(0) {
                Some("in") => Op::In,
                Some("has") => Op::Has,
                _ => {
                    self.err("expected a comparison operator");
                    return Err(());
                }
            },
            _ => {
                self.err("expected a comparison operator");
                return Err(());
            }
        };
        self.bump();
        Ok(op)
    }
}
