use super::*;

impl<'a> Parser<'a> {
    pub(super) fn model(&mut self) -> PResult<Model> {
        let start = self.here().start;
        let mut decorators = Vec::new();
        let mut scopes = Vec::new();
        while self.at(Tok::At) {
            // `@scope Name[, Name]*` is a distinct decorator form (bare names, the predicate
            // lives in the `scope` decl). Every other decorator takes the generic
            // parenthesized-args form.
            if self.ident_at(1) == Some("scope") {
                scopes.push(self.scope_deco()?);
            } else {
                decorators.push(self.decorator()?);
            }
        }
        let name = self.upper_ident("model name")?;
        self.expect(Tok::LBrace, "`{`")?;
        let mut members = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            members.push(self.member()?);
        }
        let close = self.expect(Tok::RBrace, "`}`")?;
        Ok(Model {
            decorators,
            scopes,
            name,
            members,
            span: Span {
                file: self.file,
                start,
                end: close.end,
            },
        })
    }

    /// `@scope Name[, Name]*` — one scope alternative on a model. Bare names (no
    /// parenthesized predicate); commas within one decorator are an AND-conjunction,
    /// repeated decorators are OR-alternatives.
    fn scope_deco(&mut self) -> PResult<ScopeRef> {
        let at = self.expect(Tok::At, "`@`")?;
        if !self.eat_kw("scope") {
            self.err("expected `@scope`");
            return Err(());
        }
        let mut names = vec![self.upper_ident("scope name")?];
        while self.eat(Tok::Comma) {
            names.push(self.upper_ident("scope name")?);
        }
        let end = names.last().map_or(at.end, |n| n.span.end);
        Ok(ScopeRef {
            names,
            span: Span {
                file: self.file,
                start: at.start,
                end,
            },
        })
    }
}
