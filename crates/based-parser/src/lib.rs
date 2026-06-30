//! based-parser — lexer + hand-written recursive-descent parser.
//!
//! One extension (`.bsl`), one uniform grammar: any top-level declaration may
//! appear in any file (grammar.ebnf). Hand-written (not generated) for
//! error-message quality.

use based_ast::{FileId, SchemaFile};
use based_diagnostics::Diagnostic;

/// Parse one `.bsl` source into its ordered declarations.
pub fn parse_file(_src: &str, _file: FileId) -> Result<SchemaFile, Vec<Diagnostic>> {
    todo!("parser — parser milestone")
}
