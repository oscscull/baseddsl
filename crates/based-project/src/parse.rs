//! Parse every source, preserving all parser diagnostics and stable source IDs.
use crate::Sources;
use based_ast::{Decl, FileId};
use based_diagnostics::Diagnostic;

pub(super) fn declarations(sources: &Sources) -> (Vec<Decl>, Vec<Diagnostic>) {
    let mut declarations = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, (_, text)) in sources.iter().enumerate() {
        match based_parser::parse_file(text, FileId(index as u32)) {
            Ok(file) => declarations.extend(file.decls),
            Err(errors) => diagnostics.extend(errors),
        }
    }
    (declarations, diagnostics)
}
