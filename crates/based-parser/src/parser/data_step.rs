use super::*;

impl<'a> Parser<'a> {
    /// One migration data step: `create` / `update` / `delete` over a snapshot table, with an
    /// optional trailing `chunk` modifier.
    pub(super) fn data_step(&mut self) -> PResult<DataStep> {
        let start = self.here().start;
        let op = if self.eat_kw("create") {
            self.data_create()?
        } else if self.eat_kw("update") {
            let (table, where_) = self.data_target()?;
            let assigns = self.data_assign_block()?;
            DataOp::Update {
                table,
                where_,
                assigns,
            }
        } else if self.eat_kw("delete") {
            let (table, where_) = self.data_target()?;
            DataOp::Delete { table, where_ }
        } else {
            self.err("expected a data step (`create`, `update`, or `delete`)");
            return Err(());
        };
        let chunk = self.chunk_modifier()?;
        Ok(DataStep {
            op,
            chunk,
            span: self.span_from(start),
        })
    }

    /// `create t { … }` or `create t[] from [ {…}, … ]`, then an optional `on conflict`.
    fn data_create(&mut self) -> PResult<DataOp> {
        let table = self.any_ident("a table name")?;
        let bulk = self.eat(Tok::LBracket);
        let rows = if bulk {
            self.expect(Tok::RBracket, "`]` (`create t[] from [ … ]`)")?;
            self.expect_kw("from", "`from [ … ]` after `create t[]`")?;
            self.expect(Tok::LBracket, "`[` opening the row list")?;
            let mut rows = Vec::new();
            loop {
                self.skip_seps();
                if self.at(Tok::RBracket) || self.peek().is_none() {
                    break;
                }
                let row_start = self.here().start;
                let assigns = self.data_assign_block()?;
                rows.push(DataRow {
                    assigns,
                    span: self.span_from(row_start),
                });
            }
            self.expect(Tok::RBracket, "`]` closing the row list")?;
            rows
        } else {
            let row_start = self.here().start;
            let assigns = self.data_assign_block()?;
            vec![DataRow {
                assigns,
                span: self.span_from(row_start),
            }]
        };
        let conflict = self.data_conflict()?;
        Ok(DataOp::Create {
            table,
            rows,
            bulk,
            conflict,
        })
    }

    /// `all t` (whole table) or `t where (…)`.
    fn data_target(&mut self) -> PResult<(Ident, Option<Predicate>)> {
        if self.eat_kw("all") {
            return Ok((self.any_ident("a table name")?, None));
        }
        let table = self.any_ident("a table name")?;
        if !self.eat_kw("where") {
            self.err("expected `where (…)` to filter, or `all` for the whole table");
            return Err(());
        }
        self.expect(Tok::LParen, "`(`")?;
        let pred = self.predicate()?;
        self.expect(Tok::RParen, "`)`")?;
        Ok((table, Some(pred)))
    }

    fn data_assign_block(&mut self) -> PResult<Vec<DataAssign>> {
        self.expect(Tok::LBrace, "`{`")?;
        let mut assigns = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            assigns.push(self.data_assign()?);
        }
        self.expect(Tok::RBrace, "`}`")?;
        Ok(assigns)
    }

    fn data_assign(&mut self) -> PResult<DataAssign> {
        let col = self.any_ident("a column")?;
        self.expect(Tok::Eq, "`=`")?;
        let value = self.shape_expr()?;
        Ok(DataAssign { col, value })
    }

    /// `on conflict (cols) update { col = expr, ...incoming }`.
    fn data_conflict(&mut self) -> PResult<Option<DataConflict>> {
        if !self.at_kw("on") {
            return Ok(None);
        }
        let start = self.here().start;
        self.bump();
        self.expect_kw("conflict", "`conflict` after `on`")?;
        self.expect(Tok::LParen, "`(`")?;
        let mut target = Vec::new();
        loop {
            target.push(self.any_ident("a conflict column")?);
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        self.expect_kw("update", "`update` after `on conflict (…)`")?;
        self.expect(Tok::LBrace, "`{`")?;
        let mut update = Vec::new();
        let mut spread = None;
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            if self.at(Tok::DotDotDot) {
                let s = self.here().start;
                self.bump();
                let source = self.lower_ident("`incoming`")?;
                if spread.is_some() {
                    self.err("duplicate `...incoming`");
                    return Err(());
                }
                spread = Some(ConflictSpread {
                    source,
                    preceding: update.len(),
                    span: self.span_from(s),
                });
                continue;
            }
            update.push(self.data_assign()?);
        }
        self.expect(Tok::RBrace, "`}`")?;
        Ok(Some(DataConflict {
            target,
            update,
            spread,
            span: self.span_from(start),
        }))
    }

    /// `chunk N [by col] [pause <n>ms|<n>s]`.
    fn chunk_modifier(&mut self) -> PResult<Option<Chunk>> {
        if !self.at_kw("chunk") {
            return Ok(None);
        }
        let start = self.here().start;
        self.bump();
        let size = self.positive_int("a chunk size (rows per batch)")?;
        let by = if self.eat_kw("by") {
            Some(self.any_ident("a column to walk by")?)
        } else {
            None
        };
        let pause_ms = if self.eat_kw("pause") {
            let n = self.positive_int("a pause duration (`200ms`, `2s`)")?;
            let factor = match self.ident_at(0) {
                Some("ms") => 1,
                Some("s") => 1000,
                _ => {
                    self.err("expected a duration unit (`ms` or `s`)");
                    return Err(());
                }
            };
            self.bump();
            Some(n.saturating_mul(factor))
        } else {
            None
        };
        Ok(Some(Chunk {
            size,
            by,
            pause_ms,
            span: self.span_from(start),
        }))
    }

    fn positive_int(&mut self, what: &str) -> PResult<u64> {
        let l = self.expect(Tok::Int, what)?;
        match self.text(l).parse::<u64>() {
            Ok(n) if n > 0 => Ok(n),
            _ => {
                self.pos -= 1;
                self.err(format!("expected {what} greater than zero"));
                Err(())
            }
        }
    }
}
