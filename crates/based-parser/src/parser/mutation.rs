use super::*;

impl<'a> Parser<'a> {
    pub(super) fn mutation(&mut self) -> PResult<Mutation> {
        let start = self.here().start;
        self.eat_kw("mutation");
        let name = self.lower_ident("mutation name")?;
        let params = self.param_list()?;
        self.expect(Tok::Arrow, "`->`")?;
        let ret = self.ret_type()?;
        let guard = if self.eat_kw("guard") {
            Some(self.lower_ident("guard name")?)
        } else {
            None
        };
        let (scoped, unscoped) = self.scope_ack()?;
        self.expect(Tok::LBrace, "`{`")?;
        let mut body = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            body.push(self.write_stmt()?);
        }
        let close = self.expect(Tok::RBrace, "`}`")?;
        Ok(Mutation {
            name,
            params,
            ret,
            guard,
            scoped,
            unscoped,
            body,
            span: Span {
                file: self.file,
                start,
                end: close.end,
            },
        })
    }

    fn write_stmt(&mut self) -> PResult<WriteStmt> {
        if self.eat_kw("create") {
            self.create_stmt()
        } else if self.eat_kw("update") {
            let model = self.upper_ident("model")?;
            let where_ = self.where_clause()?;
            let assigns = self.assign_block()?;
            Ok(WriteStmt::Update {
                model,
                where_,
                assigns,
            })
        } else if self.eat_kw("delete") {
            let (model, where_) = self.delete_target()?;
            Ok(WriteStmt::Delete { model, where_ })
        } else if self.eat_kw("restore") {
            let model = self.upper_ident("model")?;
            let where_ = self.where_clause()?;
            Ok(WriteStmt::Restore { model, where_ })
        } else if self.at_kw("hard") {
            self.bump();
            if !self.eat_kw("delete") {
                self.err("expected `delete` after `hard`");
                return Err(());
            }
            let (model, where_) = self.delete_target()?;
            Ok(WriteStmt::HardDelete { model, where_ })
        } else if self.eat_kw("tx") {
            self.tx_block()
        } else if self.is_raw_start() {
            Ok(WriteStmt::Raw(self.raw_sql()?))
        } else {
            self.err(
                "expected a write statement (create/update/delete/restore/hard delete/tx/raw)",
            );
            Err(())
        }
    }

    /// A `create` statement (the `create` keyword already consumed): either an assign
    /// block `{ col = … }`, or the structured shape-input form `create Model[] from $rows`
    /// (bulk) / `create Model from $row` (single), each with an optional `on conflict`
    /// upsert tail and an optional `as <name>` step binding.
    fn create_stmt(&mut self) -> PResult<WriteStmt> {
        let model = self.upper_ident("model")?;
        // A `[]` or a `from` here selects the structured shape-input form; the row values
        // come from the named param.
        let (from, assigns) = if self.at(Tok::LBracket) || self.at_kw("from") {
            let start = self.here().start;
            let bulk = self.at(Tok::LBracket);
            if bulk {
                self.bump();
                self.expect(Tok::RBracket, "`]` (bulk `create Model[] from …`)")?;
            }
            if !self.eat_kw("from") {
                self.err("expected `from $param` after `create Model[]`");
                return Err(());
            }
            let pr = self.param_ref()?;
            if !pr.path.is_empty() {
                self.err("`create … from` takes a bare `$param`, not a field path");
                return Err(());
            }
            let span = Span {
                file: self.file,
                start,
                end: self.prev_end(),
            };
            (
                Some(CreateFrom {
                    param: pr.name,
                    bulk,
                    span,
                }),
                Vec::new(),
            )
        } else {
            (None, self.assign_block()?)
        };
        let conflict = self.on_conflict()?;
        // `as <name>` binds this step's produced row for a later `$name.field` in the tx.
        let binding = if self.eat_kw("as") {
            Some(self.lower_ident("a step binding name after `as`")?)
        } else {
            None
        };
        Ok(WriteStmt::Create {
            model,
            assigns,
            from,
            conflict,
            binding,
        })
    }

    /// A `tx { … }` block (the `tx` keyword already consumed): a nested sequence of write
    /// statements committed atomically.
    fn tx_block(&mut self) -> PResult<WriteStmt> {
        self.expect(Tok::LBrace, "`{`")?;
        let mut inner = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            inner.push(self.write_stmt()?);
        }
        self.expect(Tok::RBrace, "`}`")?;
        Ok(WriteStmt::Tx(inner))
    }

    /// The `[all] Model (where (...))` target shared by `delete` and `hard delete`. The
    /// `all` keyword is the required, greppable whole-table wipe (`delete all Model` /
    /// `hard delete all Model`, `where_` = `None`); otherwise a `where` clause is required,
    /// so a whole-table wipe is always spelled explicitly.
    fn delete_target(&mut self) -> PResult<(Ident, Option<Predicate>)> {
        let all = self.eat_kw("all");
        let model = self.upper_ident("model")?;
        if all {
            return Ok((model, None));
        }
        if !self.at_kw("where") {
            self.err("expected `where (…)` to filter, or `all` to wipe the whole table");
            return Err(());
        }
        Ok((model, Some(self.where_clause()?)))
    }

    fn where_clause(&mut self) -> PResult<Predicate> {
        if !self.eat_kw("where") {
            self.err("expected `where`");
            return Err(());
        }
        self.expect(Tok::LParen, "`(`")?;
        let pred = self.predicate()?;
        self.expect(Tok::RParen, "`)`")?;
        Ok(pred)
    }

    fn assign_block(&mut self) -> PResult<Vec<Assign>> {
        self.expect(Tok::LBrace, "`{`")?;
        let mut assigns = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            let col = self.lower_ident("column")?;
            self.expect(Tok::Eq, "`=`")?;
            let value = self.assign_rhs()?;
            assigns.push(Assign { col, value });
        }
        self.expect(Tok::RBrace, "`}`")?;
        Ok(assigns)
    }
}
