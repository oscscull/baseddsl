use super::*;

impl<'a> Parser<'a> {
    /// `enum Name { pending, paid = "PAID", low = 0, … }` — a closed set of variants,
    /// each a bare identifier with an optional `= STRING | INT` wire value. Items
    /// separate on `,` or newline, like every other block.
    pub(super) fn enum_decl(&mut self) -> PResult<EnumDecl> {
        let start = self.here().start;
        self.eat_kw("enum");
        let name = self.upper_ident("enum name")?;
        self.expect(Tok::LBrace, "`{`")?;
        let mut variants = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            variants.push(self.enum_variant()?);
        }
        let close = self.expect(Tok::RBrace, "`}`")?;
        Ok(EnumDecl {
            name,
            variants,
            span: Span {
                file: self.file,
                start,
                end: close.end,
            },
        })
    }

    /// `IDENT [ '=' ( STRING | INT ) ]` — a variant name with an optional wire value.
    fn enum_variant(&mut self) -> PResult<EnumVariant> {
        let name = self.lower_ident("enum variant")?;
        let value = if self.eat(Tok::Eq) {
            let l = self
                .peek()
                .ok_or_else(|| self.err_unit("a string or integer"))?;
            let node = match l.tok {
                Tok::Str => {
                    self.bump();
                    VariantValue::Str(unquote(self.text(l)))
                }
                Tok::Int => {
                    self.bump();
                    let n = self.text(l).parse::<i64>().map_err(|_| {
                        self.err("integer variant value out of range");
                    })?;
                    VariantValue::Int(n)
                }
                _ => {
                    self.err_unit("a string or integer variant value");
                    return Err(());
                }
            };
            Some(Spanned {
                node,
                span: Span {
                    file: self.file,
                    start: l.start,
                    end: l.end,
                },
            })
        } else {
            None
        };
        Ok(EnumVariant { name, value })
    }
}
