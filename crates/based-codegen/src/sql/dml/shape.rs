//! Locate a shape declaration by name and target model.

use super::*;

/// Find the shape body for a return. `full` is per-model, so match on `from` too.
pub(crate) fn find_shape<'a>(decls: &'a [Decl], name: &str, model: &str) -> Option<&'a Shape> {
    decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name && (name != "full" || s.from.node == model) => {
            Some(s)
        }
        _ => None,
    })
}
