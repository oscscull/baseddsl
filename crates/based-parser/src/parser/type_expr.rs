use super::*;

impl<'a> Parser<'a> {
    /// A type in a non-field position (param annotation, scope term). An opaque `raw(…)`
    /// type belongs to a model field, so it is rejected here.
    pub(super) fn type_expr(&mut self) -> PResult<TypeExpr> {
        self.type_expr_inner(false)
    }

    pub(super) fn type_expr_inner(&mut self, allow_raw: bool) -> PResult<TypeExpr> {
        if self.is_raw_spec_start() {
            return self.raw_type_expr(allow_raw);
        }
        let (base, start, base_end) = self.base_type()?;
        self.type_suffix(base, start, base_end)
    }

    /// The `raw(…)` opaque-type branch: only a model field may carry it, and it has no
    /// `?`-array combination.
    fn raw_type_expr(&mut self, allow_raw: bool) -> PResult<TypeExpr> {
        if !allow_raw {
            self.err("an opaque `raw(…)` type may only type a model field");
            return Err(());
        }
        let spec = self.raw_spec()?;
        let start = spec.span.start;
        let mut end = spec.span.end;
        let optional = self.at(Tok::Question);
        if optional {
            end = self.bump().unwrap().end;
        }
        if self.at(Tok::LBracket) {
            self.err("an opaque `raw(…)` type has no array form");
            return Err(());
        }
        Ok(TypeExpr {
            base: BaseType::Raw(spec),
            optional,
            many: false,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    /// The base type: a model reference / `Id` (UpperIdent) or a primitive (LowerIdent,
    /// with `decimal(p, s)` extending its span). Returns `(base, start, base_end)`.
    fn base_type(&mut self) -> PResult<(BaseType, u32, u32)> {
        let l = self.peek().ok_or_else(|| self.err_unit("a type"))?;
        // Most primitives end at the type keyword; `decimal(p, s)` extends past it.
        let mut base_end = l.end;
        let (base, start) = match l.tok {
            Tok::UpperIdent => {
                self.bump();
                let name = self.text(l);
                if name == "Id" {
                    (BaseType::Primitive(Primitive::Id), l.start)
                } else {
                    let id = Spanned {
                        node: name.to_string(),
                        span: self.span(l),
                    };
                    (BaseType::Model(id), l.start)
                }
            }
            Tok::LowerIdent => {
                self.bump();
                let prim = match self.text(l) {
                    "text" => Primitive::Text,
                    "int" => Primitive::Int,
                    "bool" => Primitive::Bool,
                    "timestamp" => Primitive::Timestamp,
                    "date" => Primitive::Date,
                    "time" => Primitive::Time,
                    "bytes" => Primitive::Bytes,
                    "json" => Primitive::Json,
                    "uuid" => Primitive::Uuid,
                    "ulid" => Primitive::Ulid,
                    "serial" => Primitive::Serial,
                    "float" => Primitive::Float,
                    "decimal" => {
                        let (precision, scale, end) = self.decimal_args(l.end)?;
                        base_end = end;
                        Primitive::Decimal { precision, scale }
                    }
                    _ => {
                        self.err("unknown type (expected a primitive or a model reference)");
                        return Err(());
                    }
                };
                (BaseType::Primitive(prim), l.start)
            }
            _ => {
                self.err("expected a type");
                return Err(());
            }
        };
        Ok((base, start, base_end))
    }

    /// The trailing `?` (optional) and `[]` (array) suffixes on a base type, in any order.
    fn type_suffix(&mut self, base: BaseType, start: u32, base_end: u32) -> PResult<TypeExpr> {
        let mut optional = false;
        let mut many = false;
        let mut end = base_end;
        loop {
            if self.at(Tok::Question) {
                optional = true;
                end = self.bump().unwrap().end;
            } else if self.at(Tok::LBracket) && self.tok_at(1) == Some(Tok::RBracket) {
                self.bump();
                many = true;
                end = self.bump().unwrap().end;
            } else {
                break;
            }
        }
        Ok(TypeExpr {
            base,
            optional,
            many,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    /// The optional `(p, s)` after `decimal`. Bare `decimal` (no parens) defaults to
    /// `(38, 9)`. `bare_end` is the end of the `decimal` keyword (the span end when there
    /// are no parens). Range validity (`1 ≤ s ≤ p ≤ 38`) is deferred to sema.
    fn decimal_args(&mut self, bare_end: u32) -> PResult<(u32, u32, u32)> {
        if !self.at(Tok::LParen) {
            return Ok((38, 9, bare_end));
        }
        self.bump();
        let precision = self.uint_arg("decimal precision")?;
        self.expect(Tok::Comma, "`,`")?;
        let scale = self.uint_arg("decimal scale")?;
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((precision, scale, end))
    }

    /// A non-negative integer literal argument (a decimal `p`/`s`), clamped into `u32`.
    fn uint_arg(&mut self, what: &str) -> PResult<u32> {
        let l = self.expect(Tok::Int, what)?;
        let n = self.text(l).parse::<i64>().unwrap_or(-1);
        if n < 0 {
            self.err(format!("{what} must be a non-negative integer"));
            return Err(());
        }
        Ok(n.min(i64::from(u32::MAX)) as u32)
    }
}
