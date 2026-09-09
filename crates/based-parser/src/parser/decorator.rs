use super::*;

impl<'a> Parser<'a> {
    pub(super) fn decorator(&mut self) -> PResult<Decorator> {
        let at = self.expect(Tok::At, "`@`")?;
        // Decorator name may be any ident (`soft_delete`, `sort`, `index`...).
        let name = self.any_ident("decorator name")?;
        let mut args = Vec::new();
        let mut end = name.span.end;
        if self.eat(Tok::LParen) {
            if !self.at(Tok::RParen) {
                loop {
                    args.push(self.deco_arg()?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
            }
            end = self.expect(Tok::RParen, "`)`")?.end;
        }
        Ok(Decorator {
            name,
            args,
            span: Span {
                file: self.file,
                start: at.start,
                end,
            },
        })
    }

    /// A decorator argument. The `deco_arg` alternatives overlap
    /// (`sort_term`/`predicate`/`path`/`ident`/`literal`), so scan the current
    /// argument for disambiguating tokens before committing.
    fn deco_arg(&mut self) -> PResult<DecoArg> {
        // A literal-first arg (e.g. `@table("legacy")`) begins no predicate.
        if matches!(
            self.peek().map(|l| l.tok),
            Some(Tok::Str | Tok::Int | Tok::Float)
        ) {
            return Ok(DecoArg::Lit(self.literal()?));
        }
        match self.scan_arg() {
            ArgKind::Pred => Ok(DecoArg::Pred(self.predicate()?)),
            ArgKind::Sort => Ok(DecoArg::Sort(self.sort_term()?)),
            ArgKind::Path => {
                let path = self.path()?;
                if path.segments.len() == 1 {
                    Ok(DecoArg::Ident(path.segments.into_iter().next().unwrap()))
                } else {
                    Ok(DecoArg::Path(path))
                }
            }
        }
    }

    /// Look ahead within the current decorator argument (up to its `,`/`)` at
    /// depth 0) to classify it.
    fn scan_arg(&self) -> ArgKind {
        let mut depth = 0i32;
        let mut i = 0;
        let mut sort = false;
        while let Some(t) = self.tok_at(i) {
            match t {
                Tok::LParen | Tok::LBrace | Tok::LBracket => depth += 1,
                Tok::RParen if depth == 0 => break,
                Tok::RParen | Tok::RBrace | Tok::RBracket => depth -= 1,
                Tok::Comma if depth == 0 => break,
                // comparison / boolean operators => it's a predicate
                Tok::Eq
                | Tok::Ne
                | Tok::Gt
                | Tok::Lt
                | Tok::Ge
                | Tok::Le
                | Tok::Tilde
                | Tok::RawSql => return ArgKind::Pred,
                Tok::LowerIdent => match self.ident_at(i) {
                    Some("and" | "or" | "not" | "in" | "has") => return ArgKind::Pred,
                    Some("asc" | "desc") => sort = true,
                    _ => {}
                },
                _ => {}
            }
            i += 1;
        }
        if sort {
            ArgKind::Sort
        } else {
            ArgKind::Path
        }
    }
}

enum ArgKind {
    Pred,
    Sort,
    Path,
}
