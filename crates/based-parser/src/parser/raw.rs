use super::*;

impl<'a> Parser<'a> {
    /// `raw("…")` / `raw({ postgres: "…", … })` — an opaque body in type or index
    /// position. The literal is stored verbatim; nothing here interprets it.
    pub(super) fn raw_spec(&mut self) -> PResult<RawSpec> {
        let kw = self.expect(Tok::LowerIdent, "`raw`")?;
        self.expect(Tok::LParen, "`(` (a `raw(…)` body)")?;
        let body = if self.eat(Tok::LBrace) {
            let mut entries = Vec::new();
            loop {
                let dialect = self.lower_ident("a dialect name")?;
                self.expect(Tok::Colon, "`:`")?;
                let s = self.expect(Tok::Str, "a quoted literal")?;
                entries.push(RawDialect {
                    dialect,
                    text: Spanned {
                        node: unquote(self.text(s)),
                        span: self.span(s),
                    },
                });
                if !self.eat(Tok::Comma) {
                    break;
                }
                if self.at(Tok::RBrace) {
                    break;
                }
            }
            self.expect(Tok::RBrace, "`}`")?;
            RawSpecBody::PerDialect(entries)
        } else {
            let s = self.expect(Tok::Str, "a quoted literal or a `{ dialect: \"…\" }` map")?;
            RawSpecBody::All(Spanned {
                node: unquote(self.text(s)),
                span: self.span(s),
            })
        };
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(RawSpec {
            body,
            span: Span {
                file: self.file,
                start: kw.start,
                end,
            },
        })
    }

    /// Is the cursor on a `raw(` opaque body (as opposed to the backtick raw-SQL form)?
    pub(super) fn is_raw_spec_start(&self) -> bool {
        self.at_kw("raw") && self.tok_at(1) == Some(Tok::LParen)
    }

    pub(super) fn is_raw_start(&self) -> bool {
        self.at_kw("raw") && self.tok_at(1) == Some(Tok::RawSql)
    }

    pub(super) fn raw_sql(&mut self) -> PResult<RawSql> {
        let raw_kw = self.expect(Tok::LowerIdent, "`raw`")?;
        let body = self.expect(Tok::RawSql, "a `...` raw SQL body")?;
        let span = Span {
            file: self.file,
            start: raw_kw.start,
            end: body.end,
        };
        let inner = self.text(body);
        let inner = &inner[1..inner.len().saturating_sub(1)];
        Ok(RawSql {
            parts: parse_raw_parts(inner, span),
            span,
        })
    }
}

/// Split a raw-SQL body into literal text and interpolation parts.
/// `${name.path}` binds a parameter; `{ident}` is an engine-provided value
/// (`{table}`, `{id}`). Everything else is literal text.
fn parse_raw_parts(inner: &str, span: Span) -> Vec<RawPart> {
    let bytes = inner.as_bytes();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let mk_ident = |s: &str| Spanned {
        node: s.to_string(),
        span,
    };

    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(close) = find(bytes, i + 2, b'}') {
                if !text.is_empty() {
                    parts.push(RawPart::Text(std::mem::take(&mut text)));
                }
                let raw = &inner[i + 2..close];
                let mut segs = raw.split('.').map(str::trim);
                let name = mk_ident(segs.next().unwrap_or(""));
                let path = segs.map(mk_ident).collect();
                parts.push(RawPart::Param(ParamRef {
                    name,
                    path,
                    optional: false,
                }));
                i = close + 1;
                continue;
            }
        } else if bytes[i] == b'{' {
            if let Some(close) = find(bytes, i + 1, b'}') {
                if !text.is_empty() {
                    parts.push(RawPart::Text(std::mem::take(&mut text)));
                }
                parts.push(RawPart::Engine(mk_ident(inner[i + 1..close].trim())));
                i = close + 1;
                continue;
            }
        }
        text.push(inner[i..].chars().next().unwrap());
        i += inner[i..].chars().next().unwrap().len_utf8();
    }
    if !text.is_empty() {
        parts.push(RawPart::Text(text));
    }
    parts
}

fn find(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    (from..bytes.len()).find(|&i| bytes[i] == needle)
}
