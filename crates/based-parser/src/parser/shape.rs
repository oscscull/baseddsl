use super::*;

impl<'a> Parser<'a> {
    pub(super) fn shape(&mut self) -> PResult<Shape> {
        let start = self.here().start;
        self.eat_kw("shape");
        let name = self.shape_name()?;
        if !self.eat_kw("from") {
            self.err("expected `from`");
            return Err(());
        }
        let from = self.upper_ident("source model")?;
        self.expect(Tok::LBrace, "`{`")?;
        let body = self.shape_body_fields()?;
        let close = self.expect(Tok::RBrace, "`}`")?;
        Ok(Shape {
            name,
            from,
            body,
            span: Span {
                file: self.file,
                start,
                end: close.end,
            },
        })
    }

    /// `shape_name = upper_ident | 'full'`.
    fn shape_name(&mut self) -> PResult<Ident> {
        if self.at_kw("full") {
            let l = self.bump().unwrap();
            return Ok(Spanned {
                node: "full".to_string(),
                span: self.span(l),
            });
        }
        self.upper_ident("shape name")
    }

    fn shape_body_fields(&mut self) -> PResult<Vec<ShapeField>> {
        let mut fields = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            fields.push(self.shape_field()?);
        }
        Ok(fields)
    }

    fn shape_field(&mut self) -> PResult<ShapeField> {
        // `...ShapeName` — splice another shape's fields (composition).
        if self.eat(Tok::DotDotDot) {
            let shape = self.upper_ident("shape name")?;
            return Ok(ShapeField::Spread { shape });
        }
        let name = self.lower_ident("shape field")?;
        if self.eat(Tok::Eq) {
            return self.shape_value(name);
        }
        if self.at(Tok::LBrace) {
            self.bump();
            let body = self.shape_body_fields()?;
            self.expect(Tok::RBrace, "`}`")?;
            return Ok(ShapeField::Nest { field: name, body });
        }
        if self.eat(Tok::Arrow) {
            let shape = self.upper_ident("shape name")?;
            return Ok(ShapeField::NestRef { field: name, shape });
        }
        Ok(ShapeField::Bare(name))
    }

    /// The `out = <rhs>` right-hand side of a shape field: a raw SQL rename, an aggregate,
    /// a far-side flatten (`out = path { body }`), a lone-path reach, or a computed
    /// per-row expression.
    fn shape_value(&mut self, name: Ident) -> PResult<ShapeField> {
        if self.is_raw_start() {
            return Ok(ShapeField::Rename {
                out: name,
                value: ShapeValue::Raw(self.raw_sql()?),
            });
        }
        if self.at(Tok::LowerIdent) && self.tok_at(1) == Some(Tok::LParen) && !self.at_kw("case") {
            // `out = count()` / `out = sum(total)` — an aggregate. A path holds no `(`, so
            // `ident (` in a shape value is unambiguously an aggregate call.
            return Ok(ShapeField::Rename {
                out: name,
                value: ShapeValue::Agg(self.aggregate()?),
            });
        }
        // A per-row scalar expression: arithmetic / concat / `case`, or (the common case) a
        // lone column reach that keeps its `Path` / flatten forms.
        let expr = self.shape_expr()?;
        if let Some(path) = expr.lone_path() {
            // `out = path { body }` — a far-side flattening projection (skip a junction to
            // the far side of a many-to-many). A brace body after a lone path marks it,
            // apart from a plain `out = path` reach.
            if self.at(Tok::LBrace) {
                let path = path.clone();
                self.bump();
                let body = self.shape_body_fields()?;
                self.expect(Tok::RBrace, "`}`")?;
                return Ok(ShapeField::Flatten {
                    out: name,
                    path,
                    body,
                });
            }
            return Ok(ShapeField::Rename {
                out: name,
                value: ShapeValue::Path(path.clone()),
            });
        }
        Ok(ShapeField::Rename {
            out: name,
            value: ShapeValue::Computed(expr),
        })
    }
}
