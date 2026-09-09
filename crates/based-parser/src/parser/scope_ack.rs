use super::*;

impl<'a> Parser<'a> {
    /// The per-callable scope acknowledgement: exactly one of `scoped Name[, Name]*`
    /// (accept the standing scope) or `unscoped("reason")` (opt out). Sits after the
    /// return type on a query, after any `guard` on a mutation. Both `None` is legal at
    /// the parse level; sema enforces the required-declaration rule where the target is
    /// actually scoped.
    pub(super) fn scope_ack(&mut self) -> PResult<(Option<Scoped>, Option<Unscoped>)> {
        if self.at_kw("scoped") {
            Ok((Some(self.scoped_clause()?), None))
        } else {
            Ok((None, self.unscoped_clause()?))
        }
    }

    /// `scoped Name[, Name]*` — accept the standing scope(s) injected on the target.
    /// Bare names, comma-separated (mirrors `guard name`).
    fn scoped_clause(&mut self) -> PResult<Scoped> {
        let start = self.bump().unwrap().start; // `scoped`
        let mut names = vec![self.upper_ident("scope name")?];
        while self.eat(Tok::Comma) {
            names.push(self.upper_ident("scope name")?);
        }
        let end = names.last().map_or(start, |n| n.span.end);
        Ok(Scoped {
            names,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    /// `unscoped("reason")` — the per-callable `@scope` opt-out. Sits after the return
    /// type on a query, after any `guard` on a mutation. The reason string is mandatory.
    fn unscoped_clause(&mut self) -> PResult<Option<Unscoped>> {
        if !self.at_kw("unscoped") {
            return Ok(None);
        }
        let start = self.bump().unwrap().start;
        self.expect(Tok::LParen, "`(`")?;
        let s = self.expect(Tok::Str, "a reason string (`unscoped` is never silent)")?;
        let reason = unquote(self.text(s));
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(Some(Unscoped {
            reason,
            span: Span {
                file: self.file,
                start,
                end,
            },
        }))
    }
}
