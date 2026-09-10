//! Look up a shape declaration by name in the AST.

use super::*;

/// Find a shape by name in the AST (its body drives the output schema).
pub(crate) fn find_shape<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Shape> {
    decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name => Some(s),
        _ => None,
    })
}
