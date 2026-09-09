use super::*;

impl<'a> Parser<'a> {
    /// `@fk` / `@fk("reason", on_delete: cascade, on_update: cascade)`. The reason (a
    /// leading positional string) and the `on_delete:`/`on_update:` action kwargs are all
    /// optional; a bare `@fk` carries no parens at all.
    pub(super) fn fk_annot(&mut self) -> PResult<(FkAnnot, u32)> {
        let at = self.expect(Tok::At, "`@`")?;
        self.eat_kw("fk");
        let mut reason = None;
        let mut on_delete = None;
        let mut on_update = None;
        // Default span end: the last consumed token's end, else just past `fk`.
        let mut end = self
            .toks
            .get(self.pos.saturating_sub(1))
            .map_or(at.end + 2, |l| l.end);
        if self.eat(Tok::LParen) {
            // Optional leading positional reason string.
            if self.at(Tok::Str) {
                let s = self.expect(Tok::Str, "a quoted reason")?;
                reason = Some(Spanned {
                    node: unquote(self.text(s)),
                    span: Span {
                        file: self.file,
                        start: s.start,
                        end: s.end,
                    },
                });
                self.eat(Tok::Comma);
            }
            // `on_delete:`/`on_update:` action kwargs, comma-separated.
            while !self.at(Tok::RParen) {
                let kw = self.lower_ident("`on_delete` or `on_update`")?;
                self.expect(Tok::Colon, "`:`")?;
                let action = self.lower_ident("a referential action")?;
                let sp = Spanned {
                    node: action.node.clone(),
                    span: action.span,
                };
                match kw.node.as_str() {
                    "on_delete" => on_delete = Some(sp),
                    "on_update" => on_update = Some(sp),
                    _ => {
                        self.err("expected `on_delete` or `on_update`");
                        return Err(());
                    }
                }
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
            end = self.expect(Tok::RParen, "`)`")?.end;
        }
        Ok((
            FkAnnot {
                reason,
                on_delete,
                on_update,
                span: Span {
                    file: self.file,
                    start: at.start,
                    end,
                },
            },
            end,
        ))
    }

    /// `@no_fk` / `@no_fk("reason")`.
    pub(super) fn no_fk_annot(&mut self) -> PResult<(NoFkAnnot, u32)> {
        let at = self.expect(Tok::At, "`@`")?;
        self.eat_kw("no_fk");
        let mut reason = None;
        // Default span end: the last consumed token's end, else just past `no_fk`.
        let mut end = self
            .toks
            .get(self.pos.saturating_sub(1))
            .map_or(at.end + 5, |l| l.end);
        if self.eat(Tok::LParen) {
            if self.at(Tok::Str) {
                let s = self.expect(Tok::Str, "a quoted reason")?;
                reason = Some(Spanned {
                    node: unquote(self.text(s)),
                    span: Span {
                        file: self.file,
                        start: s.start,
                        end: s.end,
                    },
                });
            }
            end = self.expect(Tok::RParen, "`)`")?.end;
        }
        Ok((
            NoFkAnnot {
                reason,
                span: Span {
                    file: self.file,
                    start: at.start,
                    end,
                },
            },
            end,
        ))
    }
}
