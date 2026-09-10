//! Reprint a named `filter` declaration: name, optional params, and its predicate body.

use based_ast::*;

use crate::*;

pub(crate) fn named_filter(f: &NamedFilter) -> String {
    let ps = if f.params.is_empty() {
        String::new()
    } else {
        format!("({})", params(&f.params))
    };
    format!("filter {}{} = {};", f.name.node, ps, predicate(&f.pred, 0))
}
