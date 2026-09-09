use super::*;

impl<'a> Parser<'a> {
    pub(super) fn named_filter(&mut self) -> PResult<NamedFilter> {
        let start = self.here().start;
        self.eat_kw("filter");
        let name = self.lower_ident("filter name")?;
        let params = if self.at(Tok::LParen) {
            self.param_list()?
        } else {
            Vec::new()
        };
        self.expect(Tok::Eq, "`=`")?;
        let pred = self.predicate()?;
        let end = self.expect(Tok::Semi, "`;`")?.end;
        Ok(NamedFilter {
            name,
            params,
            pred,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }
}
