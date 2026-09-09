use super::*;

impl<'a> Parser<'a> {
    /// `count()` / `sum(total)` — an aggregate call in a shape value. The function name
    /// is any lower ident (sema restricts it to the closed set); the argument is a single
    /// optional column path (arg-less for `count()`).
    pub(super) fn aggregate(&mut self) -> PResult<AggCall> {
        let func = self.lower_ident("aggregate function")?;
        let start = func.span.start;
        self.expect(Tok::LParen, "`(`")?;
        let arg = if self.at(Tok::RParen) {
            None
        } else {
            Some(self.path()?)
        };
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(AggCall {
            func,
            arg,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }
}
