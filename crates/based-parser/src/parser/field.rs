use super::*;

impl<'a> Parser<'a> {
    pub(super) fn field_after_colon(&mut self, name: Ident) -> PResult<Field> {
        let start = name.span.start;
        let ty = self.type_expr_inner(true)?;
        let mut inverse = None;
        let mut modifiers = Vec::new();
        let mut relation_on = None;
        let mut sort = None;
        let mut was = None;
        let mut fk = None;
        let mut no_fk = None;
        let mut end = ty.span.end;

        loop {
            // `@fk` / `@fk("reason", on_delete: cascade, on_update: cascade)`.
            if self.at(Tok::At) && self.ident_at(1) == Some("fk") {
                let (annot, e) = self.fk_annot()?;
                end = e;
                fk = Some(annot);
                continue;
            }
            // `@no_fk` / `@no_fk("reason")`.
            if self.at(Tok::At) && self.ident_at(1) == Some("no_fk") {
                let (annot, e) = self.no_fk_annot()?;
                end = e;
                no_fk = Some(annot);
                continue;
            }
            // `@was("old_col")` — the field's previous physical column name.
            if self.at(Tok::At) && self.ident_at(1) == Some("was") {
                let (w, e) = self.was_annot()?;
                was = Some(w);
                end = e;
                continue;
            }
            // `@sort(term, …)` — the field's ordering (a to-many relation's default sort).
            if self.at(Tok::At) && self.ident_at(1) == Some("sort") {
                let (terms, e) = self.field_sort()?;
                sort = Some(terms);
                end = e;
                continue;
            }
            if self.at(Tok::LParen) {
                match self.paren_field_opt() {
                    ParenOpt::Inverse => {
                        let (iv, e) = self.inverse_ref()?;
                        inverse = Some(iv);
                        end = e;
                    }
                    ParenOpt::RelationOn => {
                        let (pred, e) = self.relation_opts()?;
                        relation_on = Some(pred);
                        end = e;
                    }
                    ParenOpt::Modifiers => {
                        let (mods, e) = self.modifiers()?;
                        modifiers.extend(mods);
                        end = e;
                    }
                }
                continue;
            }
            break;
        }

        Ok(Field {
            name,
            ty,
            inverse,
            modifiers,
            relation_on,
            sort,
            was,
            fk,
            no_fk,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    /// `@was("old_col")` — the field's previous physical column name.
    fn was_annot(&mut self) -> PResult<(Spanned<String>, u32)> {
        self.bump(); // @
        self.bump(); // was
        self.expect(Tok::LParen, "`(`")?;
        let s = self.expect(Tok::Str, "a quoted previous column name")?;
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((
            Spanned {
                node: unquote(self.text(s)),
                span: Span {
                    file: self.file,
                    start: s.start,
                    end: s.end,
                },
            },
            end,
        ))
    }

    /// `@sort(term, …)` — the field's ordering terms.
    fn field_sort(&mut self) -> PResult<(Vec<SortTerm>, u32)> {
        self.bump(); // @
        self.bump(); // sort
        self.expect(Tok::LParen, "`(`")?;
        let mut terms = Vec::new();
        loop {
            terms.push(self.sort_term()?);
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((terms, end))
    }

    /// Classify a `(`-led field suffix by its first inner token.
    fn paren_field_opt(&self) -> ParenOpt {
        match self.tok_at(1) {
            Some(Tok::UpperIdent) => ParenOpt::Inverse,
            _ if self.ident_at(1) == Some("on") && self.tok_at(2) == Some(Tok::Colon) => {
                ParenOpt::RelationOn
            }
            _ => ParenOpt::Modifiers,
        }
    }

    fn inverse_ref(&mut self) -> PResult<(InverseRef, u32)> {
        self.expect(Tok::LParen, "`(`")?;
        let model = self.upper_ident("inverse model")?;
        self.expect(Tok::Dot, "`.`")?;
        let field = self.lower_ident("inverse field")?;
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((InverseRef { model, field }, end))
    }

    fn relation_opts(&mut self) -> PResult<(Predicate, u32)> {
        self.expect(Tok::LParen, "`(`")?;
        self.eat_kw("on");
        self.expect(Tok::Colon, "`:`")?;
        let pred = self.predicate()?;
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((pred, end))
    }

    fn modifiers(&mut self) -> PResult<(Vec<Modifier>, u32)> {
        self.expect(Tok::LParen, "`(`")?;
        let mut mods = Vec::new();
        loop {
            let m = if self.eat_kw("unique") {
                Modifier::Unique
            } else if self.eat_kw("default") {
                Modifier::Default(self.default_val()?)
            } else if self.eat_kw("column") {
                let s = self.expect(Tok::Str, "a quoted column name")?;
                Modifier::Column(unquote(self.text(s)))
            } else {
                self.err("expected `unique`, `default`, or `column`");
                return Err(());
            };
            mods.push(m);
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((mods, end))
    }
}

enum ParenOpt {
    Inverse,
    RelationOn,
    Modifiers,
}
