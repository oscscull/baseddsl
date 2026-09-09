use super::*;

impl<'a> Parser<'a> {
    pub(super) fn member(&mut self) -> PResult<Member> {
        if self.at(Tok::At) {
            return self.index_decl().map(Member::Index);
        }
        let name = self.lower_ident("field name")?;
        // `name = <expr>` — a generated column: a stored derived value over the row's own
        // columns, reusing the shape computed-expression grammar. `:` starts an ordinary
        // field; `=` starts a generated one.
        if self.at(Tok::Eq) {
            let start = name.span.start;
            self.bump();
            let expr = self.shape_expr()?;
            return Ok(Member::Generated(GeneratedField {
                name,
                expr,
                span: self.span_from(start),
            }));
        }
        self.expect(Tok::Colon, "`:`")?;
        // `restore:`/`delete:`/`read:` followed by raw SQL is a soft-delete override.
        if self.is_raw_start() {
            let op = match name.node.as_str() {
                "restore" => SoftOp::Restore,
                "delete" => SoftOp::Delete,
                "read" => SoftOp::Read,
                _ => {
                    self.err("only `restore`, `delete`, or `read` may take a raw SQL override");
                    return Err(());
                }
            };
            let raw = self.raw_sql()?;
            return Ok(Member::SoftOverride(SoftOverride { op, raw }));
        }
        self.field_after_colon(name).map(Member::Field)
    }

    fn index_decl(&mut self) -> PResult<IndexDecl> {
        let at = self.expect(Tok::At, "`@`")?;
        if !self.eat_kw("index") {
            self.err("expected `@index` (only index declarations use `@` inside a model body)");
            return Err(());
        }
        let mut columns = Vec::new();
        let mut unique = false;
        let mut method = None;
        let mut raw = None;
        if self.is_raw_spec_start() {
            raw = Some(self.raw_spec()?);
        } else {
            if self.eat(Tok::LParen) {
                loop {
                    columns.push(self.lower_ident("index column")?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RParen, "`)`")?;
            } else {
                columns.push(self.lower_ident("index column")?);
            }
            unique = self.eat_kw("unique");
            if self.eat_kw("using") {
                method = Some(self.lower_ident("an index access method")?);
            }
        }
        let end = self
            .toks
            .get(self.pos.saturating_sub(1))
            .map_or(at.end, |l| l.end);
        Ok(IndexDecl {
            columns,
            unique,
            method,
            raw,
            span: Span {
                file: self.file,
                start: at.start,
                end,
            },
        })
    }
}
