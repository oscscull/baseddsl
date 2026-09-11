//! Reprint a `scope` declaration and the `scoped`/`unscoped` acknowledgement that
//! trails a query or mutation signature.

use crate::*;

pub(crate) fn scope_decl(s: &ScopeDecl) -> String {
    format!(
        "scope {} ({})",
        s.name.node,
        s.terms
            .iter()
            .map(|t| format!(
                "{}: {} = {}",
                t.col.node,
                type_expr(&t.ty),
                param_ref(&t.ctx)
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(crate) fn scope_ack(scoped: Option<&Scoped>, unscoped: Option<&Unscoped>) -> String {
    if let Some(s) = scoped {
        format!(
            " scoped {}",
            s.names
                .iter()
                .map(|n| n.node.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else if let Some(u) = unscoped {
        format!(" unscoped(\"{}\")", esc(&u.reason))
    } else {
        String::new()
    }
}
