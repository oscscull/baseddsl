//! Hand-written recursive-descent parser; each `parse_*` corresponds to a grammar
//! production. Hand-written (not generated) for error-message quality.
//!
//! Separators (`,` `;` and newlines) are insignificant between items: the block
//! loops skip them and never require them. Keywords are matched positionally by
//! identifier text — see `lexer.rs` — so a legacy field named `order:` parses
//! where a field is expected.

use based_ast::*;
use based_diagnostics::Diagnostic;

use crate::lexer::{lex, Lexed, Tok};

type PResult<T> = Result<T, ()>;

/// Parse one source file into its declarations. Recovers at declaration
/// boundaries so a single bad decl doesn't hide the rest; all diagnostics are
/// returned together. `Err` iff at least one diagnostic was produced.
pub fn parse(src: &str, file: FileId) -> Result<SchemaFile, Vec<Diagnostic>> {
    let lexing = lex(src);
    let mut p = Parser {
        src,
        file,
        toks: lexing.tokens,
        pos: 0,
        diags: Vec::new(),
    };
    for (start, end) in lexing.errors {
        p.diags
            .push(Diagnostic::error("E0001", "unexpected character").at(Span { file, start, end }));
    }

    let mut decls = Vec::new();
    loop {
        p.skip_seps();
        if p.peek().is_none() {
            break;
        }
        match p.decl() {
            Ok(d) => decls.push(d),
            Err(()) => p.sync(),
        }
    }

    if p.diags.is_empty() {
        Ok(SchemaFile { decls })
    } else {
        Err(p.diags)
    }
}

struct Parser<'a> {
    src: &'a str,
    file: FileId,
    toks: Vec<Lexed>,
    pos: usize,
    diags: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    // ---------- token cursor ----------------------------------------------

    fn peek(&self) -> Option<Lexed> {
        self.toks.get(self.pos).copied()
    }
    fn tok_at(&self, i: usize) -> Option<Tok> {
        self.toks.get(self.pos + i).map(|l| l.tok)
    }
    fn text(&self, l: Lexed) -> &'a str {
        &self.src[l.start as usize..l.end as usize]
    }
    /// Text of the token `i` ahead, if it is a `LowerIdent`.
    fn ident_at(&self, i: usize) -> Option<&'a str> {
        let l = self.toks.get(self.pos + i)?;
        (l.tok == Tok::LowerIdent).then(|| self.text(*l))
    }
    fn span(&self, l: Lexed) -> Span {
        Span {
            file: self.file,
            start: l.start,
            end: l.end,
        }
    }
    /// Span covering the current token, or a zero-width span at EOF.
    fn here(&self) -> Span {
        match self.peek() {
            Some(l) => self.span(l),
            None => {
                let end = self.src.len() as u32;
                Span {
                    file: self.file,
                    start: end,
                    end,
                }
            }
        }
    }
    fn bump(&mut self) -> Option<Lexed> {
        let l = self.peek();
        if l.is_some() {
            self.pos += 1;
        }
        l
    }
    fn at(&self, t: Tok) -> bool {
        self.peek().map(|l| l.tok) == Some(t)
    }
    /// Current token is a `LowerIdent` whose text equals `kw` (a positional keyword).
    fn at_kw(&self, kw: &str) -> bool {
        self.ident_at(0) == Some(kw)
    }
    fn eat(&mut self, t: Tok) -> bool {
        if self.at(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    /// Consume a positional keyword if present.
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn err(&mut self, msg: impl Into<String>) {
        let span = self.here();
        self.diags.push(Diagnostic::error("E0002", msg).at(span));
    }

    fn expect(&mut self, t: Tok, what: &str) -> PResult<Lexed> {
        if let Some(l) = self.peek() {
            if l.tok == t {
                self.pos += 1;
                return Ok(l);
            }
        }
        self.err(format!("expected {what}"));
        Err(())
    }

    fn skip_seps(&mut self) {
        while self.at(Tok::Comma) || self.at(Tok::Semi) {
            self.pos += 1;
        }
    }

    /// Error recovery: advance to the next plausible declaration start.
    fn sync(&mut self) {
        // Always make progress past the offending token.
        self.bump();
        loop {
            match self.peek() {
                None => break,
                Some(l) => {
                    if l.tok == Tok::At
                        || l.tok == Tok::UpperIdent
                        || matches!(
                            self.ident_at(0),
                            Some("shape" | "scope" | "enum" | "query" | "mutation" | "filter")
                        )
                    {
                        break;
                    }
                    self.pos += 1;
                }
            }
        }
    }

    // ---------- identifiers ------------------------------------------------

    fn lower_ident(&mut self, what: &str) -> PResult<Ident> {
        let l = self.expect(Tok::LowerIdent, what)?;
        Ok(Spanned {
            node: self.text(l).to_string(),
            span: self.span(l),
        })
    }
    fn upper_ident(&mut self, what: &str) -> PResult<Ident> {
        let l = self.expect(Tok::UpperIdent, what)?;
        Ok(Spanned {
            node: self.text(l).to_string(),
            span: self.span(l),
        })
    }

    // ---------- declarations ----------------------------------------------

    fn decl(&mut self) -> PResult<Decl> {
        if self.at(Tok::At) || self.at(Tok::UpperIdent) {
            return self.model().map(Decl::Model);
        }
        match self.ident_at(0) {
            Some("shape") => self.shape().map(Decl::Shape),
            Some("scope") => self.scope_decl().map(Decl::Scope),
            Some("enum") => self.enum_decl().map(Decl::Enum),
            Some("query") => self.query().map(Decl::Query),
            Some("mutation") => self.mutation().map(Decl::Mutation),
            Some("filter") => self.named_filter().map(Decl::Filter),
            _ => {
                self.err("expected a declaration (model, shape, query, mutation, or filter)");
                Err(())
            }
        }
    }

    // ---------- models -----------------------------------------------------

    fn model(&mut self) -> PResult<Model> {
        let start = self.here().start;
        let mut decorators = Vec::new();
        let mut scopes = Vec::new();
        while self.at(Tok::At) {
            // `@scope Name[, Name]*` is a distinct decorator form (bare names, no
            // predicate — the predicate lives in the `scope` decl). Every other
            // decorator takes the generic parenthesized-args form.
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
        let end = names.last().map(|n| n.span.end).unwrap_or(at.end);
        Ok(ScopeRef {
            names,
            span: Span {
                file: self.file,
                start: at.start,
                end,
            },
        })
    }

    // ---------- scope declarations ----------------------------------------

    /// `scope Name (col: Type = $ctx.field, …)` — a named row-visibility contract.
    /// Each term declares a scope column, its type (the one place the `$ctx` field's
    /// type is written), and the `$ctx.field` it binds.
    fn scope_decl(&mut self) -> PResult<ScopeDecl> {
        let start = self.here().start;
        self.eat_kw("scope");
        let name = self.upper_ident("scope name")?;
        self.expect(Tok::LParen, "`(`")?;
        let mut terms = Vec::new();
        if !self.at(Tok::RParen) {
            loop {
                terms.push(self.scope_term()?);
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(ScopeDecl {
            name,
            terms,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    fn scope_term(&mut self) -> PResult<ScopeTerm> {
        let col = self.lower_ident("scope column")?;
        self.expect(Tok::Colon, "`:`")?;
        let ty = self.type_expr()?;
        self.expect(Tok::Eq, "`=` (a scope term is `col: Type = $ctx.field`)")?;
        let ctx = self.param_ref()?;
        let end = ctx.path.last().map(|s| s.span.end).unwrap_or(ty.span.end);
        Ok(ScopeTerm {
            span: Span {
                file: self.file,
                start: col.span.start,
                end,
            },
            col,
            ty,
            ctx,
        })
    }

    // ---------- enum declarations -----------------------------------------

    /// `enum Name { pending, paid = "PAID", low = 0, … }` — a closed set of variants,
    /// each a bare identifier with an optional `= STRING | INT` wire value. Items
    /// separate on `,` or newline, like every other block.
    fn enum_decl(&mut self) -> PResult<EnumDecl> {
        let start = self.here().start;
        self.eat_kw("enum");
        let name = self.upper_ident("enum name")?;
        self.expect(Tok::LBrace, "`{`")?;
        let mut variants = Vec::new();
        loop {
            self.skip_seps();
            if self.at(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            variants.push(self.enum_variant()?);
        }
        let close = self.expect(Tok::RBrace, "`}`")?;
        Ok(EnumDecl {
            name,
            variants,
            span: Span {
                file: self.file,
                start,
                end: close.end,
            },
        })
    }

    /// `IDENT [ '=' ( STRING | INT ) ]` — a variant name with an optional wire value.
    fn enum_variant(&mut self) -> PResult<EnumVariant> {
        let name = self.lower_ident("enum variant")?;
        let value = if self.eat(Tok::Eq) {
            let l = self
                .peek()
                .ok_or_else(|| self.err_unit("a string or integer"))?;
            let node = match l.tok {
                Tok::Str => {
                    self.bump();
                    VariantValue::Str(unquote(self.text(l)))
                }
                Tok::Int => {
                    self.bump();
                    let n = self.text(l).parse::<i64>().map_err(|_| {
                        self.err("integer variant value out of range");
                    })?;
                    VariantValue::Int(n)
                }
                _ => {
                    self.err_unit("a string or integer variant value");
                    return Err(());
                }
            };
            Some(Spanned {
                node,
                span: Span {
                    file: self.file,
                    start: l.start,
                    end: l.end,
                },
            })
        } else {
            None
        };
        Ok(EnumVariant { name, value })
    }

    fn decorator(&mut self) -> PResult<Decorator> {
        let at = self.expect(Tok::At, "`@`")?;
        // Decorator name may be any ident (`soft_delete`, `sort`, `index`...).
        let name = self.any_ident("decorator name")?;
        let mut args = Vec::new();
        let mut end = name.span.end;
        if self.eat(Tok::LParen) {
            if !self.at(Tok::RParen) {
                loop {
                    args.push(self.deco_arg()?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
            }
            end = self.expect(Tok::RParen, "`)`")?.end;
        }
        Ok(Decorator {
            name,
            args,
            span: Span {
                file: self.file,
                start: at.start,
                end,
            },
        })
    }

    /// A decorator argument. The grammar's `deco_arg` alternatives overlap
    /// (`sort_term`/`predicate`/`path`/`ident`/`literal`), so we scan the current
    /// argument for disambiguating tokens before committing.
    fn deco_arg(&mut self) -> PResult<DecoArg> {
        // Literal-first args (e.g. `@table("legacy")`) can't begin a predicate.
        if matches!(
            self.peek().map(|l| l.tok),
            Some(Tok::Str | Tok::Int | Tok::Float)
        ) {
            return Ok(DecoArg::Lit(self.literal()?));
        }
        match self.scan_arg() {
            ArgKind::Pred => Ok(DecoArg::Pred(self.predicate()?)),
            ArgKind::Sort => Ok(DecoArg::Sort(self.sort_term()?)),
            ArgKind::Path => {
                let path = self.path()?;
                if path.segments.len() == 1 {
                    Ok(DecoArg::Ident(path.segments.into_iter().next().unwrap()))
                } else {
                    Ok(DecoArg::Path(path))
                }
            }
        }
    }

    /// Look ahead within the current decorator argument (up to its `,`/`)` at
    /// depth 0) to classify it.
    fn scan_arg(&self) -> ArgKind {
        let mut depth = 0i32;
        let mut i = 0;
        let mut sort = false;
        while let Some(t) = self.tok_at(i) {
            match t {
                Tok::LParen | Tok::LBrace | Tok::LBracket => depth += 1,
                Tok::RParen if depth == 0 => break,
                Tok::RParen | Tok::RBrace | Tok::RBracket => depth -= 1,
                Tok::Comma if depth == 0 => break,
                // comparison / boolean operators => it's a predicate
                Tok::Eq
                | Tok::Ne
                | Tok::Gt
                | Tok::Lt
                | Tok::Ge
                | Tok::Le
                | Tok::Tilde
                | Tok::RawSql => return ArgKind::Pred,
                Tok::LowerIdent => match self.ident_at(i) {
                    Some("and" | "or" | "not" | "in" | "has") => return ArgKind::Pred,
                    Some("asc" | "desc") => sort = true,
                    _ => {}
                },
                _ => {}
            }
            i += 1;
        }
        if sort {
            ArgKind::Sort
        } else {
            ArgKind::Path
        }
    }

    fn member(&mut self) -> PResult<Member> {
        if self.at(Tok::At) {
            return self.index_decl().map(Member::Index);
        }
        let name = self.lower_ident("field name")?;
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
        let unique = self.eat_kw("unique");
        let end = self
            .toks
            .get(self.pos.saturating_sub(1))
            .map(|l| l.end)
            .unwrap_or(at.end);
        Ok(IndexDecl {
            columns,
            unique,
            span: Span {
                file: self.file,
                start: at.start,
                end,
            },
        })
    }

    fn field_after_colon(&mut self, name: Ident) -> PResult<Field> {
        let start = name.span.start;
        let ty = self.type_expr()?;
        let mut inverse = None;
        let mut modifiers = Vec::new();
        let mut relation_on = None;
        let mut sort = None;
        let mut was = None;
        let mut end = ty.span.end;

        loop {
            // `@was("old_col")` — the field's previous physical column name.
            if self.at(Tok::At) && self.ident_at(1) == Some("was") {
                self.bump(); // @
                self.bump(); // was
                self.expect(Tok::LParen, "`(`")?;
                let s = self.expect(Tok::Str, "a quoted previous column name")?;
                let end_p = self.expect(Tok::RParen, "`)`")?.end;
                was = Some(Spanned {
                    node: unquote(self.text(s)),
                    span: Span {
                        file: self.file,
                        start: s.start,
                        end: s.end,
                    },
                });
                end = end_p;
                continue;
            }
            if self.at(Tok::At) && self.ident_at(1) == Some("sort") {
                self.bump(); // @
                self.bump(); // sort
                self.expect(Tok::LParen, "`(`")?;
                let mut terms = Vec::new();
                loop {
                    terms.push(self.sort_term()?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
                end = self.expect(Tok::RParen, "`)`")?.end;
                sort = Some(terms);
                continue;
            }
            if self.at(Tok::LParen) {
                match self.paren_field_opt() {
                    ParenOpt::Inverse => {
                        let (iv, e) = self.inverse_ref()?;
                        inverse = Some(iv);
                        end = e;
                    }
                    ParenOpt::RelationOn => {
                        let (pred, e) = self.relation_opts()?;
                        relation_on = Some(pred);
                        end = e;
                    }
                    ParenOpt::Modifiers => {
                        let (mods, e) = self.modifiers()?;
                        modifiers.extend(mods);
                        end = e;
                    }
                }
                continue;
            }
            break;
        }

        Ok(Field {
            name,
            ty,
            inverse,
            modifiers,
            relation_on,
            sort,
            was,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    /// Classify a `(`-led field suffix by its first inner token.
    fn paren_field_opt(&self) -> ParenOpt {
        match self.tok_at(1) {
            Some(Tok::UpperIdent) => ParenOpt::Inverse,
            _ if self.ident_at(1) == Some("on") && self.tok_at(2) == Some(Tok::Colon) => {
                ParenOpt::RelationOn
            }
            _ => ParenOpt::Modifiers,
        }
    }

    fn inverse_ref(&mut self) -> PResult<(InverseRef, u32)> {
        self.expect(Tok::LParen, "`(`")?;
        let model = self.upper_ident("inverse model")?;
        self.expect(Tok::Dot, "`.`")?;
        let field = self.lower_ident("inverse field")?;
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((InverseRef { model, field }, end))
    }

    fn relation_opts(&mut self) -> PResult<(Predicate, u32)> {
        self.expect(Tok::LParen, "`(`")?;
        self.eat_kw("on");
        self.expect(Tok::Colon, "`:`")?;
        let pred = self.predicate()?;
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((pred, end))
    }

    fn modifiers(&mut self) -> PResult<(Vec<Modifier>, u32)> {
        self.expect(Tok::LParen, "`(`")?;
        let mut mods = Vec::new();
        loop {
            let m = if self.eat_kw("unique") {
                Modifier::Unique
            } else if self.eat_kw("default") {
                Modifier::Default(self.default_val()?)
            } else if self.eat_kw("column") {
                let s = self.expect(Tok::Str, "a quoted column name")?;
                Modifier::Column(unquote(self.text(s)))
            } else {
                self.err("expected `unique`, `default`, or `column`");
                return Err(());
            };
            mods.push(m);
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok((mods, end))
    }

    /// The optional `(p, s)` after `decimal`. Bare `decimal` (no parens) defaults to
    /// `(38, 9)`. `bare_end` is the end of the `decimal` keyword (the span end when there
    /// are no parens). Range validity (`1 ≤ s ≤ p ≤ 38`) is a sema check, not a parse one.
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
        Ok(n.min(u32::MAX as i64) as u32)
    }

    fn type_expr(&mut self) -> PResult<TypeExpr> {
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
                    "json" => Primitive::Json,
                    "uuid" => Primitive::Uuid,
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

    // ---------- shapes -----------------------------------------------------

    fn shape(&mut self) -> PResult<Shape> {
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
        let name = self.lower_ident("shape field")?;
        if self.eat(Tok::Eq) {
            let value = if self.is_raw_start() {
                ShapeValue::Raw(self.raw_sql()?)
            } else {
                ShapeValue::Path(self.path()?)
            };
            return Ok(ShapeField::Rename { out: name, value });
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

    // ---------- queries ----------------------------------------------------

    fn query(&mut self) -> PResult<Query> {
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

    /// The per-callable scope acknowledgement: exactly one of `scoped Name[, Name]*`
    /// (accept the standing scope) or `unscoped("reason")` (opt out). Sits after the
    /// return type on a query, after any `guard` on a mutation. Both `None` is legal at
    /// the parse level; sema enforces the required-declaration rule where the target is
    /// actually scoped.
    fn scope_ack(&mut self) -> PResult<(Option<Scoped>, Option<Unscoped>)> {
        if self.at_kw("scoped") {
            Ok((Some(self.scoped_clause()?), None))
        } else {
            Ok((None, self.unscoped_clause()?))
        }
    }

    /// `scoped Name[, Name]*` — accept the standing scope(s) injected on the target.
    /// Bare names, comma-separated (mirrors `guard name`).
    fn scoped_clause(&mut self) -> PResult<Scoped> {
        let start = self.bump().unwrap().start; // `scoped`
        let mut names = vec![self.upper_ident("scope name")?];
        while self.eat(Tok::Comma) {
            names.push(self.upper_ident("scope name")?);
        }
        let end = names.last().map(|n| n.span.end).unwrap_or(start);
        Ok(Scoped {
            names,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    /// `unscoped("reason")` — the per-callable `@scope` opt-out. Sits after the return
    /// type on a query, after any `guard` on a mutation. The reason string is mandatory
    /// (an escape hatch is never silent).
    fn unscoped_clause(&mut self) -> PResult<Option<Unscoped>> {
        if !self.at_kw("unscoped") {
            return Ok(None);
        }
        let start = self.bump().unwrap().start;
        self.expect(Tok::LParen, "`(`")?;
        let s = self.expect(Tok::Str, "a reason string (`unscoped` is never silent)")?;
        let reason = unquote(self.text(s));
        let end = self.expect(Tok::RParen, "`)`")?.end;
        Ok(Some(Unscoped {
            reason,
            span: Span {
                file: self.file,
                start,
                end,
            },
        }))
    }

    fn ret_type(&mut self) -> PResult<RetType> {
        // `stream` is contextual: a keyword only in return-type position.
        let stream = self.eat_kw("stream");
        // `ok` is contextual too: the shapeless acknowledgement a destructive
        // mutation returns (mutations.md). Bare — no `[]`, no `stream`.
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
        // Whole-query raw body (raw.md): the block is one `raw`…`` statement.
        if self.is_raw_start() {
            let raw = self.raw_sql()?;
            self.skip_seps(); // the statement-terminating `;`
            let end = self.expect(Tok::RBrace, "`}`")?.end;
            return Ok((QueryBody::Raw(raw), end));
        }
        let verb = if self.eat_kw("get") {
            Verb::Get
        } else if self.eat_kw("list") {
            Verb::List
        } else {
            self.err("expected `get`, `list`, or a `raw` body");
            return Err(());
        };
        let model = self.upper_ident("model")?;
        let mut clauses = Vec::new();
        while self.at_clause() {
            clauses.push(self.clause()?);
        }
        self.skip_seps(); // the statement-terminating `;`
        let end = self.expect(Tok::RBrace, "`}`")?.end;
        Ok((
            QueryBody::Block(Statement {
                verb,
                model,
                clauses,
            }),
            end,
        ))
    }

    fn at_clause(&self) -> bool {
        self.at_kw("where") || self.at_kw("order") || self.at_kw("page") || self.at_kw("unindexed")
    }

    fn clause(&mut self) -> PResult<Clause> {
        if self.eat_kw("where") {
            self.expect(Tok::LParen, "`(`")?;
            let pred = self.predicate()?;
            self.expect(Tok::RParen, "`)`")?;
            Ok(Clause::Where(pred))
        } else if self.eat_kw("order") {
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
        } else if self.eat_kw("page") {
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
        } else if self.at_kw("unindexed") {
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
        } else {
            self.err("expected `where`, `order`, `page`, or `unindexed`");
            Err(())
        }
    }

    // ---------- mutations --------------------------------------------------

    fn mutation(&mut self) -> PResult<Mutation> {
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
            let model = self.upper_ident("model")?;
            let assigns = self.assign_block()?;
            Ok(WriteStmt::Create { model, assigns })
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
            let model = self.upper_ident("model")?;
            let where_ = self.where_clause()?;
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
            let model = self.upper_ident("model")?;
            let where_ = self.where_clause()?;
            Ok(WriteStmt::HardDelete { model, where_ })
        } else if self.eat_kw("tx") {
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
        } else if self.is_raw_start() {
            Ok(WriteStmt::Raw(self.raw_sql()?))
        } else {
            self.err(
                "expected a write statement (create/update/delete/restore/hard delete/tx/raw)",
            );
            Err(())
        }
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

    // ---------- named filters ---------------------------------------------

    fn named_filter(&mut self) -> PResult<NamedFilter> {
        let start = self.here().start;
        self.eat_kw("filter");
        let name = self.lower_ident("filter name")?;
        let params = if self.at(Tok::LParen) {
            self.param_list()?
        } else {
            Vec::new()
        };
        self.expect(Tok::Eq, "`=`")?;
        let pred = self.predicate()?;
        let end = self.expect(Tok::Semi, "`;`")?.end;
        Ok(NamedFilter {
            name,
            params,
            pred,
            span: Span {
                file: self.file,
                start,
                end,
            },
        })
    }

    // ---------- params -----------------------------------------------------

    fn param_list(&mut self) -> PResult<Vec<Param>> {
        self.expect(Tok::LParen, "`(`")?;
        let mut params = Vec::new();
        if !self.at(Tok::RParen) {
            loop {
                params.push(self.param()?);
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        Ok(params)
    }

    fn param(&mut self) -> PResult<Param> {
        let name = self.lower_ident("parameter name")?;
        let ty = if self.eat(Tok::Colon) {
            Some(self.type_expr()?)
        } else {
            None
        };
        // binding: `-> edge`, or a non-`=` comparison op + column. `=` is a default.
        let binding = if self.eat(Tok::Arrow) {
            Some(ParamBinding::Edge(self.lower_ident("edge field")?))
        } else if self.at_binding_op() {
            let op = self.op()?;
            let col = self.lower_ident("column")?;
            Some(ParamBinding::ColOp { op, col })
        } else {
            None
        };
        let default = if self.eat(Tok::Eq) {
            Some(self.default_val()?)
        } else {
            None
        };
        Ok(Param {
            name,
            ty,
            binding,
            default,
        })
    }

    /// A comparison operator that begins a param binding (`=` is excluded — it
    /// introduces a default, and same-name equality is the binding default).
    fn at_binding_op(&self) -> bool {
        matches!(
            self.peek().map(|l| l.tok),
            Some(Tok::Ne | Tok::Gt | Tok::Lt | Tok::Ge | Tok::Le | Tok::Tilde)
        ) || matches!(self.ident_at(0), Some("in" | "has"))
    }

    // ---------- predicates -------------------------------------------------

    fn predicate(&mut self) -> PResult<Predicate> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> PResult<Predicate> {
        let mut lhs = self.and_expr()?;
        while self.eat_kw("or") {
            let rhs = self.and_expr()?;
            lhs = Predicate::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> PResult<Predicate> {
        let mut lhs = self.not_expr()?;
        while self.eat_kw("and") {
            let rhs = self.not_expr()?;
            lhs = Predicate::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn not_expr(&mut self) -> PResult<Predicate> {
        if self.eat_kw("not") {
            Ok(Predicate::Not(Box::new(self.atom()?)))
        } else {
            self.atom()
        }
    }

    fn atom(&mut self) -> PResult<Predicate> {
        if self.eat(Tok::LParen) {
            let p = self.predicate()?;
            self.expect(Tok::RParen, "`)`")?;
            return Ok(p);
        }
        if self.is_raw_start() {
            return Ok(Predicate::Raw(self.raw_sql()?));
        }
        // A leading lower ident is a path, a comparison, or a filter call.
        let first = self.lower_ident("a column, filter, or predicate")?;
        if self.at(Tok::LParen) {
            // filter call: `name(args)`
            self.bump();
            let mut args = Vec::new();
            if !self.at(Tok::RParen) {
                loop {
                    args.push(self.value()?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
            }
            self.expect(Tok::RParen, "`)`")?;
            return Ok(Predicate::FilterCall { name: first, args });
        }
        let path = self.path_from(first);
        if self.at_op() {
            let op = self.op()?;
            // `in (` opens a value list; `in` with a bare value stays a plain Cmp.
            if op == Op::In && self.eat(Tok::LParen) {
                let mut values = Vec::new();
                loop {
                    values.push(self.value()?);
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RParen, "`)`")?;
                return Ok(Predicate::InList { path, values });
            }
            let value = self.value()?;
            Ok(Predicate::Cmp { path, op, value })
        } else {
            // bare filter reference or a bool column, both `Bare(path)`
            Ok(Predicate::Bare(path))
        }
    }

    fn at_op(&self) -> bool {
        matches!(
            self.peek().map(|l| l.tok),
            Some(Tok::Eq | Tok::Ne | Tok::Gt | Tok::Lt | Tok::Ge | Tok::Le | Tok::Tilde)
        ) || matches!(self.ident_at(0), Some("in" | "has"))
    }

    fn op(&mut self) -> PResult<Op> {
        let op = match self.peek().map(|l| l.tok) {
            Some(Tok::Eq) => Op::Eq,
            Some(Tok::Ne) => Op::Ne,
            Some(Tok::Gt) => Op::Gt,
            Some(Tok::Lt) => Op::Lt,
            Some(Tok::Ge) => Op::Ge,
            Some(Tok::Le) => Op::Le,
            Some(Tok::Tilde) => Op::Like,
            Some(Tok::LowerIdent) => match self.ident_at(0) {
                Some("in") => Op::In,
                Some("has") => Op::Has,
                _ => {
                    self.err("expected a comparison operator");
                    return Err(());
                }
            },
            _ => {
                self.err("expected a comparison operator");
                return Err(());
            }
        };
        self.bump();
        Ok(op)
    }

    // ---------- values / paths / literals ---------------------------------

    fn value(&mut self) -> PResult<Value> {
        match self.peek().map(|l| l.tok) {
            Some(Tok::Dollar) => Ok(Value::Param(self.param_ref()?)),
            Some(Tok::Caret) => Ok(Value::Back(self.back_ref()?)),
            Some(Tok::Int | Tok::Float | Tok::Str) => Ok(Value::Lit(self.literal()?)),
            Some(Tok::LowerIdent) => match self.ident_at(0) {
                Some("true") | Some("false") | Some("null") => Ok(Value::Lit(self.literal()?)),
                _ if self.tok_at(1) == Some(Tok::LParen) => Ok(Value::Func(self.func_call()?)),
                _ => Ok(Value::Path(self.path()?)),
            },
            _ => {
                self.err("expected a value ($param, path, literal, or function call)");
                Err(())
            }
        }
    }

    /// An assignment RHS: a plain `value`, or a scalar arithmetic expression over the
    /// target model's numeric columns + params (`total = total + $n`, mutations.md).
    /// `+`/`-` are the lowest-precedence, left-associative level.
    fn assign_rhs(&mut self) -> PResult<AssignRhs> {
        let start = self.here().start;
        let mut lhs = self.arith_term()?;
        loop {
            let op = if self.at(Tok::Plus) {
                ArithOp::Add
            } else if self.at(Tok::Minus) {
                ArithOp::Sub
            } else {
                break;
            };
            self.bump();
            let rhs = self.arith_term()?;
            lhs = self.mk_arith(lhs, op, rhs, start);
        }
        Ok(lhs)
    }

    /// `*`/`/` — the tighter-binding, left-associative arithmetic level.
    fn arith_term(&mut self) -> PResult<AssignRhs> {
        let start = self.here().start;
        let mut lhs = self.arith_factor()?;
        loop {
            let op = if self.at(Tok::Star) {
                ArithOp::Mul
            } else if self.at(Tok::Slash) {
                ArithOp::Div
            } else {
                break;
            };
            self.bump();
            let rhs = self.arith_factor()?;
            lhs = self.mk_arith(lhs, op, rhs, start);
        }
        Ok(lhs)
    }

    /// A leaf of an arithmetic RHS: a `value`, or a parenthesized subexpression.
    fn arith_factor(&mut self) -> PResult<AssignRhs> {
        if self.at(Tok::LParen) {
            self.bump();
            let inner = self.assign_rhs()?;
            self.expect(Tok::RParen, "`)`")?;
            Ok(inner)
        } else {
            Ok(AssignRhs::Value(self.value()?))
        }
    }

    fn mk_arith(&self, lhs: AssignRhs, op: ArithOp, rhs: AssignRhs, start: u32) -> AssignRhs {
        AssignRhs::Arith {
            lhs: Box::new(lhs),
            op,
            rhs: Box::new(rhs),
            span: Span {
                file: self.file,
                start,
                end: self.prev_end(),
            },
        }
    }

    /// End offset of the most-recently consumed token (for spans that close on it).
    fn prev_end(&self) -> u32 {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.toks.get(i))
            .map(|l| l.end)
            .unwrap_or(0)
    }

    /// `^.field` — a tx back-reference. One field segment: the reference is to a
    /// just-created row's column (`^.id` for FK wiring).
    fn back_ref(&mut self) -> PResult<BackRef> {
        let caret = self.expect(Tok::Caret, "`^`")?;
        self.expect(Tok::Dot, "`.` after `^`")?;
        let field = self.lower_ident("field name")?;
        let span = Span {
            file: self.file,
            start: caret.start,
            end: field.span.end,
        };
        Ok(BackRef { field, span })
    }

    fn param_ref(&mut self) -> PResult<ParamRef> {
        self.expect(Tok::Dollar, "`$`")?;
        let name = self.lower_ident("parameter name")?;
        let mut path = Vec::new();
        while self.eat(Tok::Dot) {
            path.push(self.lower_ident("path segment")?);
        }
        Ok(ParamRef { name, path })
    }

    fn path(&mut self) -> PResult<Path> {
        let first = self.lower_ident("a path")?;
        Ok(self.path_from(first))
    }

    fn path_from(&mut self, first: Ident) -> Path {
        let mut segments = vec![first];
        while self.eat(Tok::Dot) {
            match self.lower_ident("path segment") {
                Ok(seg) => segments.push(seg),
                Err(()) => break,
            }
        }
        Path { segments }
    }

    fn func_call(&mut self) -> PResult<FuncCall> {
        let name = self.lower_ident("function name")?;
        self.expect(Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(Tok::RParen) {
            loop {
                args.push(self.value()?);
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(Tok::RParen, "`)`")?;
        Ok(FuncCall { name, args })
    }

    fn default_val(&mut self) -> PResult<DefaultVal> {
        if self.at(Tok::LowerIdent) && self.tok_at(1) == Some(Tok::LParen) {
            // func default e.g. now()
            return Ok(DefaultVal::Func(self.func_call()?));
        }
        // A bare identifier that isn't a `true`/`false`/`null` literal is an enum
        // variant default (`default pending`); sema checks it against the column's enum.
        if self.at(Tok::LowerIdent) && !matches!(self.ident_at(0), Some("true" | "false" | "null"))
        {
            return Ok(DefaultVal::Variant(self.lower_ident("default variant")?));
        }
        Ok(DefaultVal::Lit(self.literal()?))
    }

    fn literal(&mut self) -> PResult<Literal> {
        let l = self.peek().ok_or_else(|| self.err_unit("a literal"))?;
        let lit = match l.tok {
            Tok::Int => {
                let n = self.text(l).parse::<i64>().map_err(|_| {
                    self.err("integer literal out of range");
                })?;
                Literal::Int(n)
            }
            // A fractional literal is kept as its exact source text (see `Literal::Decimal`)
            // so a `decimal` default / value never rounds through an `f64`.
            Tok::Float => Literal::Decimal(self.text(l).to_string()),
            Tok::Str => Literal::Str(unquote(self.text(l))),
            Tok::LowerIdent => match self.text(l) {
                "true" => Literal::Bool(true),
                "false" => Literal::Bool(false),
                "null" => Literal::Null,
                _ => {
                    self.err("expected a literal");
                    return Err(());
                }
            },
            _ => {
                self.err("expected a literal");
                return Err(());
            }
        };
        self.bump();
        Ok(lit)
    }

    fn int_lit(&mut self) -> PResult<i64> {
        let l = self.expect(Tok::Int, "an integer")?;
        self.text(l).parse::<i64>().map_err(|_| {
            self.err("integer literal out of range");
        })
    }

    fn sort_term(&mut self) -> PResult<SortTerm> {
        let path = self.path()?;
        let dir = if self.eat_kw("desc") {
            SortDir::Desc
        } else {
            self.eat_kw("asc");
            SortDir::Asc
        };
        Ok(SortTerm { path, dir })
    }

    // ---------- raw SQL ----------------------------------------------------

    fn is_raw_start(&self) -> bool {
        self.at_kw("raw") && self.tok_at(1) == Some(Tok::RawSql)
    }

    fn raw_sql(&mut self) -> PResult<RawSql> {
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

    // ---------- small helpers ---------------------------------------------

    /// Any identifier (used for decorator names, which may be lower- or upper-cased).
    fn any_ident(&mut self, what: &str) -> PResult<Ident> {
        match self.peek() {
            Some(l) if l.tok == Tok::LowerIdent || l.tok == Tok::UpperIdent => {
                self.bump();
                Ok(Spanned {
                    node: self.text(l).to_string(),
                    span: self.span(l),
                })
            }
            _ => {
                self.err(format!("expected {what}"));
                Err(())
            }
        }
    }

    fn err_unit(&mut self, what: &str) {
        self.err(format!("expected {what}"));
    }
}

enum ArgKind {
    Pred,
    Sort,
    Path,
}

enum ParenOpt {
    Inverse,
    RelationOn,
    Modifiers,
}

/// Strip surrounding quotes and unescape `\"` / `\\` from a `STRING` slice.
fn unquote(s: &str) -> String {
    let inner = &s[1..s.len().saturating_sub(1)];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(esc) => out.push(esc),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
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
                parts.push(RawPart::Param(ParamRef { name, path }));
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
