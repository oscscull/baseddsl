use super::*;

impl<'a> Parser<'a> {
    pub(super) fn query(&mut self) -> PResult<Query> {
        let start = self.here().start;
        self.eat_kw("query");
        let name = self.lower_ident("query name")?;
        let params = self.param_list()?;
        self.expect(Tok::Arrow, "`->`")?;
        let ret = self.ret_type()?;
        let (scoped, unscoped) = self.scope_ack()?;

        let (body, end) = if self.at(Tok::Semi) {
            let e = self.bump().unwrap().end;
            (QueryBody::Bare, e)
        } else if self.at(Tok::LBrace) {
            self.query_block()?
        } else {
            // inline tail clauses on an otherwise-bare query
            let mut clauses = Vec::new();
            while self.at_clause() {
                clauses.push(self.clause()?);
            }
            let e = self.expect(Tok::Semi, "`;`")?.end;
            (QueryBody::Inline(clauses), e)
        };

        Ok(Query {
            name,
            params,
            ret,
            scoped,
            unscoped,
            body,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    pub(super) fn ret_type(&mut self) -> PResult<RetType> {
        // `stream` is contextual: a keyword only in return-type position.
        let stream = self.eat_kw("stream");
        // `ok` is contextual too: the shapeless acknowledgement a destructive mutation
        // returns, a lone token.
        if !stream && self.at_kw("ok") {
            let l = self.bump().unwrap();
            if self.at(Tok::LBracket) {
                self.err("`ok` is a bare acknowledgement — drop the `[]`");
                return Err(());
            }
            return Ok(RetType {
                ty: Spanned {
                    node: "ok".to_string(),
                    span: self.span(l),
                },
                many: false,
                stream: false,
                ack: true,
            });
        }
        let ty = if self.at_kw("full") {
            let l = self.bump().unwrap();
            Spanned {
                node: "full".to_string(),
                span: self.span(l),
            }
        } else {
            self.upper_ident("return type (a shape or model)")?
        };
        let many = self.at(Tok::LBracket) && self.tok_at(1) == Some(Tok::RBracket);
        if many {
            if stream {
                self.err("`stream` already means many — drop the `[]`");
                return Err(());
            }
            self.bump();
            self.bump();
        }
        Ok(RetType {
            ty,
            many,
            stream,
            ack: false,
        })
    }

    fn query_block(&mut self) -> PResult<(QueryBody, u32)> {
        self.expect(Tok::LBrace, "`{`")?;
        // Whole-query raw body: the block is one `raw` … `` statement.
        if self.is_raw_start() {
            let raw = self.raw_sql()?;
            self.skip_seps(); // the statement-terminating `;`
            let end = self.expect(Tok::RBrace, "`}`")?.end;
            return Ok((QueryBody::Raw(raw), end));
        }
        let (verb, distinct) = if self.eat_kw("get") {
            (Verb::Get, false)
        } else if self.eat_kw("list") {
            // `list distinct <M>` — dedup projected rows. `distinct` pairs with `list` only.
            (Verb::List, self.eat_kw("distinct"))
        } else {
            self.err("expected `get`, `list`, or a `raw` body");
            return Err(());
        };
        let model = self.upper_ident("model")?;
        let mut clauses = Vec::new();
        while self.at_clause() {
            clauses.push(self.clause()?);
        }
        // `for update[ nowait| skip locked]` — a trailing locking-read modifier after the
        // clause list. `for update` is one compound keyword, mirroring the SQL it lowers to;
        // an optional wait mode follows.
        let for_update = if self.eat_kw("for") {
            self.expect_kw("update", "`update` (the `for update` locking modifier)")?;
            let wait = if self.eat_kw("nowait") {
                LockWait::NoWait
            } else if self.eat_kw("skip") {
                self.expect_kw("locked", "`locked` (the `for update skip locked` modifier)")?;
                LockWait::SkipLocked
            } else {
                LockWait::Wait
            };
            Some(wait)
        } else {
            None
        };
        self.skip_seps(); // the statement-terminating `;`
        let end = self.expect(Tok::RBrace, "`}`")?.end;
        Ok((
            QueryBody::Block(Statement {
                verb,
                model,
                clauses,
                distinct,
                for_update,
            }),
            end,
        ))
    }

    fn at_clause(&self) -> bool {
        self.at_kw("where")
            || self.at_kw("order")
            || self.at_kw("page")
            || self.at_kw("unindexed")
            || self.at_kw("group")
            || self.at_kw("having")
    }

    fn clause(&mut self) -> PResult<Clause> {
        if self.eat_kw("where") {
            self.expect(Tok::LParen, "`(`")?;
            let pred = self.predicate()?;
            self.expect(Tok::RParen, "`)`")?;
            Ok(Clause::Where(pred))
        } else if self.eat_kw("order") {
            self.order_clause()
        } else if self.eat_kw("page") {
            self.page_clause()
        } else if self.eat_kw("group") {
            self.group_by_clause()
        } else if self.eat_kw("having") {
            self.having_clause()
        } else if self.at_kw("unindexed") {
            self.unindexed_clause()
        } else {
            self.err("expected `where`, `order`, `page`, or `unindexed`");
            Err(())
        }
    }

    /// `order (term, …)` — the sort cascade.
    fn order_clause(&mut self) -> PResult<Clause> {
        self.expect(Tok::LParen, "`(`")?;
        let mut terms = Vec::new();
        loop {
            terms.push(self.sort_term()?);
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        Ok(Clause::Order(terms))
    }

    /// `page (N) [offset] [with count]` — pagination.
    fn page_clause(&mut self) -> PResult<Clause> {
        self.expect(Tok::LParen, "`(`")?;
        let n = self.int_lit()?;
        self.expect(Tok::RParen, "`)`")?;
        let offset = self.eat_kw("offset");
        let with_count = self.eat_kw("with") && self.eat_kw("count");
        Ok(Clause::Page(PageClause {
            size: n as u64,
            offset,
            with_count,
        }))
    }

    /// `group by (col, …)` — aggregate grouping.
    fn group_by_clause(&mut self) -> PResult<Clause> {
        self.expect_kw("by", "`by`")?;
        self.expect(Tok::LParen, "`(`")?;
        let mut cols = Vec::new();
        loop {
            cols.push(self.path()?);
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        Ok(Clause::GroupBy(cols))
    }

    /// `having (pred)` — a post-grouping filter.
    fn having_clause(&mut self) -> PResult<Clause> {
        self.expect(Tok::LParen, "`(`")?;
        let pred = self.predicate()?;
        self.expect(Tok::RParen, "`)`")?;
        Ok(Clause::Having(pred))
    }

    /// `unindexed (max_rows: N | unsafe[, "reason"])` — the un-indexed-scan escape hatch.
    fn unindexed_clause(&mut self) -> PResult<Clause> {
        let start = self.bump().unwrap().start;
        self.expect(Tok::LParen, "`(`")?;
        let kind = if self.eat_kw("unsafe") {
            let reason = if self.eat(Tok::Comma) {
                let s = self.expect(Tok::Str, "a reason string")?;
                Some(unquote(self.text(s)))
            } else {
                None
            };
            UnindexedKind::Unsafe(reason)
        } else if self.eat_kw("max_rows") {
            self.expect(Tok::Colon, "`:`")?;
            UnindexedKind::MaxRows(self.int_lit()? as u64)
        } else {
            self.err("expected `max_rows: N` or `unsafe`");
            return Err(());
        };
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(Clause::Unindexed(Unindexed {
            kind,
            span: Span {
                file: self.file,
                start,
                end,
            },
        }))
    }
}
