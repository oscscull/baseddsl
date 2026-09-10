//! based-fmt — the canonical `.bsl` source formatter.
//!
//! One entry point, [`format_source`]: parse a `.bsl` source, then pretty-print its
//! declarations in the canonical layout the worked examples use. Deterministic and
//! idempotent — `format(format(x)) == format(x)` — and structure-preserving:
//! re-parsing the output yields the same declarations.
//!
//! Comments are lexer-skipped, so the printer recovers them from the source text
//! directly: a `.bsl` comment is a full-line `#` comment, reproduced verbatim in its
//! original slot — before a declaration, between a model's decorators, or between the
//! members of a body (a model's fields, a mutation's statements, a shape's fields),
//! re-indented to the body. Comments interior to a construct the printer renders on a
//! single line (an inline `{ a = 1, b = 2 }` assign block) may reflow to the nearest
//! member boundary rather than staying mid-line.

use based_ast::*;
use based_diagnostics::Diagnostic;

mod enums;
mod expr;
mod filter;
mod index;
mod model;
mod mutation;
mod printer;
mod query;
mod raw;
mod scope;
mod shape;
mod spans;
mod types;
mod value;

use printer::Printer;

pub(crate) use enums::*;
pub(crate) use expr::*;
pub(crate) use filter::*;
pub(crate) use index::*;
pub(crate) use model::*;
pub(crate) use mutation::*;
pub(crate) use query::*;
pub(crate) use raw::*;
pub(crate) use scope::*;
pub(crate) use shape::*;
pub(crate) use spans::*;
pub(crate) use types::*;
pub(crate) use value::*;

/// Format a `.bsl` source into its canonical layout. `Err` carries the parse
/// diagnostics when the source does not parse (an unparseable file can't be
/// formatted).
pub fn format_source(src: &str) -> Result<String, Vec<Diagnostic>> {
    let parsed = based_parser::parse_file(src, FileId(0))?;
    Ok(Printer::new(src).file(&parsed.decls))
}

/// Whether a source is already canonically formatted.
pub fn is_formatted(src: &str) -> Result<bool, Vec<Diagnostic>> {
    Ok(format_source(src)? == src)
}

/// Reprint a computed/generated-column expression in canonical form (minimal
/// parentheses), e.g. `price - discount`. Used by editor surfaces (hover) to echo a
/// generated column's defining expression.
pub fn reprint_expr(e: &ShapeExpr) -> String {
    shape_expr(e, 0)
}
