use super::*;

impl<'a> Parser<'a> {
    /// `scope Name (col: Type = $ctx.field, …)` — a named row-visibility contract.
    /// Each term declares a scope column, its type (the one place the `$ctx` field's
    /// type is written), and the `$ctx.field` it binds.
    pub(super) fn scope_decl(&mut self) -> PResult<ScopeDecl> {
        let start = self.here().start;
        self.eat_kw("scope");
        let name = self.upper_ident("scope name")?;
        self.expect(Tok::LParen, "`(`")?;
        let mut terms = Vec::new();
        if !self.at(Tok::RParen) {
            loop {
                terms.push(self.scope_term()?);
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(ScopeDecl {
            name,
            terms,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    fn scope_term(&mut self) -> PResult<ScopeTerm> {
        let col = self.lower_ident("scope column")?;
        self.expect(Tok::Colon, "`:`")?;
        let ty = self.type_expr()?;
        self.expect(Tok::Eq, "`=` (a scope term is `col: Type = $ctx.field`)")?;
        let ctx = self.param_ref()?;
        let end = ctx.path.last().map_or(ty.span.end, |s| s.span.end);
        Ok(ScopeTerm {
            span: Span {
                file: self.file,
                start: col.span.start,
                end,
            },
            col,
            ty,
            ctx,
        })
    }
}
