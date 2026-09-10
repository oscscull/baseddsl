//! Reprint an `@index` member (single- or multi-column, optional `unique`/method,
//! or a raw index spec) and the soft-delete override keyword.

use based_ast::*;

pub(crate) fn index_decl(ix: &IndexDecl) -> String {
    if let Some(spec) = &ix.raw {
        return format!("@index {}", spec.render());
    }
    let cols = if ix.columns.len() == 1 {
        ix.columns[0].node.clone()
    } else {
        format!(
            "({})",
            ix.columns
                .iter()
                .map(|c| c.node.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let sep = if ix.columns.len() == 1 { " " } else { "" };
    let method = match &ix.method {
        Some(m) => format!(" using {}", m.node),
        None => String::new(),
    };
    format!(
        "@index{sep}{cols}{}{method}",
        if ix.unique { " unique" } else { "" }
    )
}

pub(crate) fn soft_op(op: SoftOp) -> &'static str {
    match op {
        SoftOp::Restore => "restore",
        SoftOp::Delete => "delete",
        SoftOp::Read => "read",
    }
}
