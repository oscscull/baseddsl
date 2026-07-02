//! Lexer for `.bsl`. Thin wrapper over a `logos` token set.
//!
//! Casing is load-bearing (decisions.md D7), so lower- and upper-camel
//! identifiers are distinct token kinds — the parser never re-inspects the first
//! byte. Keywords are NOT lexed as dedicated tokens: they ride in as
//! `LowerIdent` and the parser recognizes them positionally. This is what lets a
//! legacy-named field like `order:` (a D8 reserved word) parse where a field is
//! expected, while `order (...)` still reads as a clause where a clause is
//! expected — the token is the same, only the position differs.
//!
//! Whitespace, newlines and `#` comments are skipped: per grammar.ebnf they
//! separate tokens but never carry meaning (principle 3). Item separators
//! (`,` `;`) survive as tokens and are treated as optional by the parser.

use logos::Logos;

/// One lexical token kind. The source slice is recovered from the `Span`.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"[ \t\r\n\f]+")] // whitespace: insignificant
#[logos(skip r"#[^\n]*")] // line comment
pub enum Tok {
    // --- identifiers (casing distinguishes model refs from columns, D7) ---
    #[regex(r"[a-z_][a-zA-Z0-9_]*")]
    LowerIdent,
    #[regex(r"[A-Z][a-zA-Z0-9]*")]
    UpperIdent,

    // --- literals ---
    #[regex(r"[0-9]+\.[0-9]+")]
    Float,
    #[regex(r"[0-9]+")]
    Int,
    #[regex(r#""([^"\\]|\\.)*""#)]
    Str,
    /// A whole backtick-delimited raw-SQL body, backticks included. Interpolation
    /// parts are split out later (raw.md). Grammar forbids a literal backtick
    /// inside, so a single `[^`]*` run is exact.
    #[regex(r"`[^`]*`")]
    RawSql,

    // --- delimiters ---
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,

    // --- punctuation ---
    #[token(":")]
    Colon,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token(";")]
    Semi,
    #[token("@")]
    At,
    #[token("$")]
    Dollar,
    #[token("->")]
    Arrow,
    #[token("?")]
    Question,
    /// `^` — tx back-reference marker (`^.field`, mutations.md).
    #[token("^")]
    Caret,

    // --- operators (multi-char forms first so they win the longest match) ---
    #[token("!=")]
    Ne,
    #[token(">=")]
    Ge,
    #[token("<=")]
    Le,
    #[token("=")]
    Eq,
    #[token(">")]
    Gt,
    #[token("<")]
    Lt,
    #[token("~")]
    Tilde,
}

/// A token plus its half-open byte range `[start, end)` in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lexed {
    pub tok: Tok,
    pub start: u32,
    pub end: u32,
}

/// Outcome of lexing: the token stream, plus any byte offset the lexer could not
/// classify (the parser turns these into diagnostics).
pub struct Lexing {
    pub tokens: Vec<Lexed>,
    /// Byte ranges the lexer rejected (unexpected characters).
    pub errors: Vec<(u32, u32)>,
}

/// Tokenize a whole source string. Never fails; unrecognized bytes are collected
/// into `errors` and dropped so the parser can still make progress.
pub fn lex(src: &str) -> Lexing {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut lx = Tok::lexer(src);
    while let Some(res) = lx.next() {
        let span = lx.span();
        match res {
            Ok(tok) => tokens.push(Lexed {
                tok,
                start: span.start as u32,
                end: span.end as u32,
            }),
            Err(()) => errors.push((span.start as u32, span.end as u32)),
        }
    }
    Lexing { tokens, errors }
}
