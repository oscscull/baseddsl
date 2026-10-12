use super::*;

/// One entry of an `on conflict update { … }` block.
enum UpdateItem {
    Assign(DataAssign),
    Spread(ConflictSpread),
}

impl<'a> Parser<'a> {
    /// One migration data step: `create` / `update` / `delete` over a snapshot table, with an
    /// optional trailing `chunk` modifier.
    pub(super) fn data_step(&mut self) -> PResult<DataStep> {
        let start = self.here().start;
        let op = match self.ident_at(0) {
            Some("create") => {
                self.bump();
                self.data_create()?
            }
            Some("update") => {
                self.bump();
                let (table, where_) = self.data_target()?;
                let assigns = self.data_assign_block()?;
                DataOp::Update {
                    table,
                    where_,
                    assigns,
                }
            }
            Some("delete") => {
                self.bump();
                let (table, where_) = self.data_target()?;
                DataOp::Delete { table, where_ }
            }
            _ => {
                self.err("expected a data step (`create`, `update`, or `delete`)");
                return Err(());
            }
        };
        Ok(DataStep {
            op,
            chunk: self.chunk_modifier()?,
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
            let rows = self.items_until(Tok::RBracket, |p, _| p.data_row())?;
            self.expect(Tok::RBracket, "`]` closing the row list")?;
            rows
        } else {
            vec![self.data_row()?]
        };
        Ok(DataOp::Create {
            table,
            rows,
            bulk,
            conflict: self.data_conflict()?,
        })
    }

    fn data_row(&mut self) -> PResult<DataRow> {
        let start = self.here().start;
        let assigns = self.data_assign_block()?;
        Ok(DataRow {
            assigns,
            span: self.span_from(start),
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
        let assigns = self.items_until(Tok::RBrace, |p, _| p.data_assign())?;
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
        let mut target = vec![self.any_ident("a conflict column")?];
        while self.eat(Tok::Comma) {
            target.push(self.any_ident("a conflict column")?);
        }
        self.expect(Tok::RParen, "`)`")?;
        self.expect_kw("update", "`update` after `on conflict (…)`")?;
        self.expect(Tok::LBrace, "`{`")?;
        let items = self.items_until(Tok::RBrace, Self::update_item)?;
        self.expect(Tok::RBrace, "`}`")?;
        let (update, spread) = items.into_iter().fold(
            (Vec::new(), None),
            |(mut update, spread), item| match item {
                UpdateItem::Assign(a) => {
                    update.push(a);
                    (update, spread)
                }
                UpdateItem::Spread(s) => (update, Some(s)),
            },
        );
        Ok(Some(DataConflict {
            target,
            update,
            spread,
            span: self.span_from(start),
        }))
    }

    /// A `col = expr` assign, or the single `...incoming` spread positioned after `seen`.
    fn update_item(&mut self, seen: &[UpdateItem]) -> PResult<UpdateItem> {
        if !self.at(Tok::DotDotDot) {
            return self.data_assign().map(UpdateItem::Assign);
        }
        let start = self.here().start;
        self.bump();
        let source = self.lower_ident("`incoming`")?;
        if seen.iter().any(|i| matches!(i, UpdateItem::Spread(_))) {
            self.err("duplicate `...incoming`");
            return Err(());
        }
        Ok(UpdateItem::Spread(ConflictSpread {
            source,
            preceding: seen.len(),
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
        let by = self
            .eat_kw("by")
            .then(|| self.any_ident("a column to walk by"))
            .transpose()?;
        let pause_ms = self.eat_kw("pause").then(|| self.pause_ms()).transpose()?;
        Ok(Some(Chunk {
            size,
            by,
            pause_ms,
            span: self.span_from(start),
        }))
    }

    /// `<n>ms` or `<n>s`, in milliseconds.
    fn pause_ms(&mut self) -> PResult<u64> {
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
        Ok(n.saturating_mul(factor))
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

    /// Separator-delimited items up to (not consuming) `close`; `item` sees those parsed so far.
    fn items_until<T>(
        &mut self,
        close: Tok,
        mut item: impl FnMut(&mut Self, &[T]) -> PResult<T>,
    ) -> PResult<Vec<T>> {
        let mut items = Vec::new();
        loop {
            self.skip_seps();
            if self.at(close) || self.peek().is_none() {
                return Ok(items);
            }
            items.push(item(self, &items)?);
        }
    }
}
