use super::*;

/// Find a shape by name in the AST (its body drives the output struct). Names are
/// unique across shapes except `full`, which its caller resolves.
pub(super) fn find_shape<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Shape> {
    decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name => Some(s),
        _ => None,
    })
}
