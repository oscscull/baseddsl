use super::*;

impl<'a> Parser<'a> {
    /// Whether the cursor is at a date-bucket call (`date(` / `week(`).
    pub(super) fn at_bucket_call(&self) -> bool {
        self.tok_at(1) == Some(Tok::LParen) && BUCKET_FUNCS.iter().any(|f| self.at_kw(f))
    }

    /// `date(col)` / `week(col)` — a date-bucket call. Every comma-separated column path
    /// is kept so sema reports a wrong arity.
    pub(super) fn bucket_call(&mut self) -> PResult<BucketCall> {
        let func = self.lower_ident("date function")?;
        let bucket = Bucket::from_name(&func.node).unwrap_or(Bucket::Day);
        let start = func.span.start;
        self.expect(Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(Tok::RParen) {
            loop {
                args.push(self.path()?);
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(BucketCall {
            bucket,
            func,
            args,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }
}
