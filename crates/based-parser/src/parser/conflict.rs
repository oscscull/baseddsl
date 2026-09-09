use super::*;

impl<'a> Parser<'a> {
    /// The optional `on conflict (cols) update { … }` tail of a `create` (upsert). Sits
    /// directly after the create's assign block; present only when the tail opens with `on`.
    pub(super) fn on_conflict(&mut self) -> PResult<Option<OnConflict>> {
        if !self.at_kw("on") {
            return Ok(None);
        }
        let start = self.here().start;
        self.bump(); // `on`
        if !self.eat_kw("conflict") {
            self.err("expected `conflict` after `on`");
            return Err(());
        }
        self.expect(Tok::LParen, "`(`")?;
        let mut target = Vec::new();
        loop {
            target.push(self.lower_ident("conflict column")?);
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        if !self.eat_kw("update") {
            self.err("expected `update` after `on conflict (...)`");
            return Err(());
        }
        let (update, spread) = self.conflict_update_block()?;
        Ok(Some(OnConflict {
            target,
            update,
            spread,
            span: Span {
                file: self.file,
                start,
                end: self.prev_end(),
            },
        }))
    }

    /// The `update { … }` branch of an `on conflict`. Like an ordinary assign block, but it
    /// also accepts a single `...incoming` spread (bulk upsert) among the assigns; the spread
    /// and the explicit assigns are returned separately, the spread carrying its lexical
    /// position for last-write-wins ordering.
    fn conflict_update_block(&mut self) -> PResult<(Vec<Assign>, Option<ConflictSpread>)> {
        self.expect(Tok::LBrace, "`{`")?;
        let mut assigns = Vec::new();
        let mut spread: Option<ConflictSpread> = None;
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            if self.at(Tok::DotDotDot) {
                if spread.is_some() {
                    self.err("duplicate `...incoming` in an `on conflict update` branch");
                    return Err(());
                }
                let start = self.here().start;
                self.bump(); // `...`
                let source = self.lower_ident("`incoming`")?;
                spread = Some(ConflictSpread {
                    source,
                    preceding: assigns.len(),
                    span: Span {
                        file: self.file,
                        start,
                        end: self.prev_end(),
                    },
                });
                continue;
            }
            let col = self.lower_ident("column")?;
            self.expect(Tok::Eq, "`=`")?;
            let value = self.assign_rhs()?;
            assigns.push(Assign { col, value });
        }
        self.expect(Tok::RBrace, "`}`")?;
        Ok((assigns, spread))
    }
}
