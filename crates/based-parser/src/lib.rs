//! based-parser — lexer + hand-written recursive-descent parser.
//!
//! One extension (`.bsl`), one uniform grammar: any top-level declaration may
//! appear in any file. Hand-written (not generated) for error-message quality.

mod lexer;
mod parser;

pub use lexer::{lex, Lexed, Tok};

use based_ast::{FileId, SchemaFile};
use based_diagnostics::Diagnostic;

/// Parse one `.bsl` source into its ordered declarations. Recovers at
/// declaration boundaries, so `Err` carries every diagnostic found, not just the
/// first.
pub fn parse_file(src: &str, file: FileId) -> Result<SchemaFile, Vec<Diagnostic>> {
    parser::parse(src, file)
}
