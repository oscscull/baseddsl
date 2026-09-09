use super::*;

impl<'a> Parser<'a> {
    pub(super) fn value(&mut self) -> PResult<Value> {
        match self.peek().map(|l| l.tok) {
            Some(Tok::Dollar) => Ok(Value::Param(self.param_ref()?)),
            Some(Tok::Int | Tok::Float | Tok::Str) => Ok(Value::Lit(self.literal()?)),
            Some(Tok::LowerIdent) => match self.ident_at(0) {
                Some("true" | "false" | "null") => Ok(Value::Lit(self.literal()?)),
                _ if self.tok_at(1) == Some(Tok::LParen) => Ok(Value::Func(self.func_call()?)),
                _ => Ok(Value::Path(self.path()?)),
            },
            _ => {
                self.err("expected a value ($param, path, literal, or function call)");
                Err(())
            }
        }
    }

    pub(super) fn param_ref(&mut self) -> PResult<ParamRef> {
        self.expect(Tok::Dollar, "`$`")?;
        let name = self.lower_ident("parameter name")?;
        let mut path = Vec::new();
        while self.eat(Tok::Dot) {
            path.push(self.lower_ident("path segment")?);
        }
        // A trailing `?` marks an optional context read (`$ctx.user?`): the predicate
        // leaf present-guards away when the field is absent. Only valid on `$ctx.<field>`
        // in a query filter — sema rejects it in any other position.
        let optional = self.eat(Tok::Question);
        Ok(ParamRef {
            name,
            path,
            optional,
        })
    }

    fn func_call(&mut self) -> PResult<FuncCall> {
        let name = self.lower_ident("function name")?;
        self.expect(Tok::LParen, "`(`")?;
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
        Ok(FuncCall { name, args })
    }

    pub(super) fn default_val(&mut self) -> PResult<DefaultVal> {
        if self.at(Tok::LowerIdent) && self.tok_at(1) == Some(Tok::LParen) {
            // func default e.g. now()
            return Ok(DefaultVal::Func(self.func_call()?));
        }
        // A bare identifier other than `true`/`false`/`null` is an enum variant default
        // (`default pending`); sema checks it against the column's enum.
        if self.at(Tok::LowerIdent) && !matches!(self.ident_at(0), Some("true" | "false" | "null"))
        {
            return Ok(DefaultVal::Variant(self.lower_ident("default variant")?));
        }
        Ok(DefaultVal::Lit(self.literal()?))
    }

    pub(super) fn literal(&mut self) -> PResult<Literal> {
        let l = self.peek().ok_or_else(|| self.err_unit("a literal"))?;
        let lit = match l.tok {
            Tok::Int => {
                let n = self.text(l).parse::<i64>().map_err(|_| {
                    self.err("integer literal out of range");
                })?;
                Literal::Int(n)
            }
            // A fractional literal is kept as its exact source text (see `Literal::Decimal`)
            // so a `decimal` default / value keeps full precision.
            Tok::Float => Literal::Decimal(self.text(l).to_string()),
            Tok::Str => Literal::Str(unquote(self.text(l))),
            Tok::LowerIdent => match self.text(l) {
                "true" => Literal::Bool(true),
                "false" => Literal::Bool(false),
                "null" => Literal::Null,
                _ => {
                    self.err("expected a literal");
                    return Err(());
                }
            },
            _ => {
                self.err("expected a literal");
                return Err(());
            }
        };
        self.bump();
        Ok(lit)
    }

    pub(super) fn int_lit(&mut self) -> PResult<i64> {
        let l = self.expect(Tok::Int, "an integer")?;
        self.text(l).parse::<i64>().map_err(|_| {
            self.err("integer literal out of range");
        })
    }
}
