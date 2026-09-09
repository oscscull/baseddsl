use super::*;

impl<'a> Parser<'a> {
    pub(super) fn peek(&self) -> Option<Lexed> {
        self.toks.get(self.pos).copied()
    }
    pub(super) fn tok_at(&self, i: usize) -> Option<Tok> {
        self.toks.get(self.pos + i).map(|l| l.tok)
    }
    pub(super) fn text(&self, l: Lexed) -> &'a str {
        &self.src[l.start as usize..l.end as usize]
    }
    /// Text of the token `i` ahead, if it is a `LowerIdent`.
    pub(super) fn ident_at(&self, i: usize) -> Option<&'a str> {
        let l = self.toks.get(self.pos + i)?;
        (l.tok == Tok::LowerIdent).then(|| self.text(*l))
    }
    pub(super) fn span(&self, l: Lexed) -> Span {
        Span {
            file: self.file,
            start: l.start,
            end: l.end,
        }
    }
    /// Span covering the current token, or a zero-width span at EOF.
    pub(super) fn here(&self) -> Span {
        if let Some(l) = self.peek() {
            self.span(l)
        } else {
            let end = self.src.len() as u32;
            Span {
                file: self.file,
                start: end,
                end,
            }
        }
    }
    pub(super) fn bump(&mut self) -> Option<Lexed> {
        let l = self.peek();
        if l.is_some() {
            self.pos += 1;
        }
        l
    }
    pub(super) fn at(&self, t: Tok) -> bool {
        self.peek().map(|l| l.tok) == Some(t)
    }
    /// Current token is a `LowerIdent` whose text equals `kw` (a positional keyword).
    pub(super) fn at_kw(&self, kw: &str) -> bool {
        self.ident_at(0) == Some(kw)
    }
    pub(super) fn eat(&mut self, t: Tok) -> bool {
        if self.at(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    /// Consume a positional keyword if present.
    pub(super) fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Consume a required positional keyword (`group` `by`), erroring if absent.
    pub(super) fn expect_kw(&mut self, kw: &str, what: &str) -> PResult<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            self.err(format!("expected {what}"));
            Err(())
        }
    }

    pub(super) fn err(&mut self, msg: impl Into<String>) {
        let span = self.here();
        self.diags.push(Diagnostic::error("E0002", msg).at(span));
    }

    pub(super) fn expect(&mut self, t: Tok, what: &str) -> PResult<Lexed> {
        if let Some(l) = self.peek() {
            if l.tok == t {
                self.pos += 1;
                return Ok(l);
            }
        }
        self.err(format!("expected {what}"));
        Err(())
    }

    pub(super) fn skip_seps(&mut self) {
        while self.at(Tok::Comma) || self.at(Tok::Semi) {
            self.pos += 1;
        }
    }

    /// Error recovery: advance to the next plausible declaration start.
    pub(super) fn sync(&mut self) {
        // Always make progress past the offending token.
        self.bump();
        loop {
            match self.peek() {
                None => break,
                Some(l) => {
                    if l.tok == Tok::At
                        || l.tok == Tok::UpperIdent
                        || matches!(
                            self.ident_at(0),
                            Some("shape" | "scope" | "enum" | "query" | "mutation" | "filter")
                        )
                    {
                        break;
                    }
                    self.pos += 1;
                }
            }
        }
    }

    pub(super) fn lower_ident(&mut self, what: &str) -> PResult<Ident> {
        let l = self.expect(Tok::LowerIdent, what)?;
        Ok(Spanned {
            node: self.text(l).to_string(),
            span: self.span(l),
        })
    }
    pub(super) fn upper_ident(&mut self, what: &str) -> PResult<Ident> {
        let l = self.expect(Tok::UpperIdent, what)?;
        Ok(Spanned {
            node: self.text(l).to_string(),
            span: self.span(l),
        })
    }

    /// Any identifier (used for decorator names, which may be lower- or upper-cased).
    pub(super) fn any_ident(&mut self, what: &str) -> PResult<Ident> {
        match self.peek() {
            Some(l) if l.tok == Tok::LowerIdent || l.tok == Tok::UpperIdent => {
                self.bump();
                Ok(Spanned {
                    node: self.text(l).to_string(),
                    span: self.span(l),
                })
            }
            _ => {
                self.err(format!("expected {what}"));
                Err(())
            }
        }
    }

    pub(super) fn err_unit(&mut self, what: &str) {
        self.err(format!("expected {what}"));
    }

    /// End offset of the most-recently consumed token (for spans that close on it).
    pub(super) fn prev_end(&self) -> u32 {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.toks.get(i))
            .map_or(0, |l| l.end)
    }

    pub(super) fn span_from(&self, start: u32) -> Span {
        Span {
            file: self.file,
            start,
            end: self.prev_end(),
        }
    }
}
