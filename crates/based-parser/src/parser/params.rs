use super::*;

impl<'a> Parser<'a> {
    pub(super) fn param_list(&mut self) -> PResult<Vec<Param>> {
        self.expect(Tok::LParen, "`(`")?;
        let mut params = Vec::new();
        if !self.at(Tok::RParen) {
            loop {
                params.push(self.param()?);
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        Ok(params)
    }

    fn param(&mut self) -> PResult<Param> {
        let name = self.lower_ident("parameter name")?;
        // `name?` — an optional filter param (sits on the name, before the `:` type).
        let optional = self.eat(Tok::Question);
        let ty = if self.eat(Tok::Colon) {
            Some(self.type_expr()?)
        } else {
            None
        };
        // binding: `-> edge`, or a non-`=` comparison op + column. `=` is a default.
        let binding = if self.eat(Tok::Arrow) {
            Some(ParamBinding::Edge(self.lower_ident("edge field")?))
        } else if self.at_binding_op() {
            let op = self.op()?;
            let col = self.lower_ident("column")?;
            Some(ParamBinding::ColOp { op, col })
        } else {
            None
        };
        let default = if self.eat(Tok::Eq) {
            Some(self.default_val()?)
        } else {
            None
        };
        Ok(Param {
            name,
            optional,
            ty,
            binding,
            default,
        })
    }

    /// A comparison operator that begins a param binding (`=` is excluded — it
    /// introduces a default, and same-name equality is the binding default).
    fn at_binding_op(&self) -> bool {
        matches!(
            self.peek().map(|l| l.tok),
            Some(Tok::Ne | Tok::Gt | Tok::Lt | Tok::Ge | Tok::Le | Tok::Tilde)
        ) || matches!(self.ident_at(0), Some("in" | "has"))
    }
}
