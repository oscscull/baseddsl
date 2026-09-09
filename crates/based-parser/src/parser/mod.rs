//! Hand-written recursive-descent parser (for error-message quality); each `parse_*`
//! matches a grammar production.
//!
//! Separators (`,` `;` newlines) are optional between items. Keywords are positional
//! identifier text (see `lexer.rs`), so a field named `order:` parses as a field.

use based_ast::*;
use based_diagnostics::Diagnostic;

use crate::lexer::{lex, Lexed, Tok};

mod aggregate;
mod arith;
mod conflict;
mod cursor;
mod decl;
mod decorator;
mod enums;
mod field;
mod filter;
mod fk;
mod member;
mod model;
mod mutation;
mod params;
mod path;
mod predicate;
mod query;
mod raw;
mod scope;
mod scope_ack;
mod shape;
mod shape_expr;
mod sort;
mod type_expr;
mod unquote;
mod value;

pub(crate) use unquote::unquote;

pub(crate) type PResult<T> = Result<T, ()>;

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
        // `^` was the tx back-reference marker (`^.id`), removed in favour of named step
        // bindings; point the user at the replacement.
        let msg = if src[start as usize..end as usize].contains('^') {
            "`^` tx back-references were removed — bind the step with `create … as <name>;` and reference it as `$name.field`"
        } else {
            "unexpected character"
        };
        p.diags
            .push(Diagnostic::error("E0001", msg).at(Span { file, start, end }));
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

/// Parse a standalone shape / generated-column expression (`price - discount`, `a || b`,
/// `case when … then … else … end`). Used to re-lower a persisted generated-column
/// expression per dialect at migration time. `Err` iff the input is not exactly one
/// expression (a lex error, a parse error, or trailing tokens).
pub fn parse_expr(src: &str, file: FileId) -> Result<ShapeExpr, Vec<Diagnostic>> {
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
    match p.shape_expr() {
        Ok(e) if p.diags.is_empty() && p.peek().is_none() => Ok(e),
        Ok(_) => {
            if p.peek().is_some() {
                p.diags.push(
                    Diagnostic::error("E0002", "trailing tokens after expression").at(p.here()),
                );
            }
            Err(p.diags)
        }
        Err(()) => Err(p.diags),
    }
}

struct Parser<'a> {
    src: &'a str,
    file: FileId,
    toks: Vec<Lexed>,
    pos: usize,
    diags: Vec<Diagnostic>,
}
