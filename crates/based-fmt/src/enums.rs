//! Reprint an enum declaration on one line, variants comma-joined, preserving
//! explicit string/int variant values.

use based_ast::*;

use crate::*;

/// `enum Name { a, b = "B", c }` — one line, variants comma-joined (a closed value set
/// reads as a compact list). An explicit variant value (`= "PAID"` / `= 0`) is preserved;
/// a bare variant stays bare.
pub(crate) fn enum_decl(e: &EnumDecl) -> String {
    format!(
        "enum {} {{ {} }}",
        e.name.node,
        e.variants
            .iter()
            .map(enum_variant)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn enum_variant(v: &EnumVariant) -> String {
    match v.value.as_ref().map(|s| &s.node) {
        None => v.name.node.clone(),
        Some(VariantValue::Str(s)) => format!("{} = \"{}\"", v.name.node, esc(s)),
        Some(VariantValue::Int(n)) => format!("{} = {}", v.name.node, n),
    }
}
