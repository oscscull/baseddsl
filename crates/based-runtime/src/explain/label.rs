use based_ast::Param;

/// The source of one placeholder's value: a declared `$param`, an optional filter's
/// presence flag, a `$ctx` field, or a value the engine supplies.
pub(super) fn param_label(params: &[Param], placeholder: &str) -> String {
    let declared = |name: &str| params.iter().any(|p| p.name.node == name);
    if declared(placeholder) {
        return format!("${placeholder}");
    }
    if let Some(name) = placeholder.strip_suffix("__present") {
        if declared(name) {
            return format!("${name} present (1/0)");
        }
    }
    if let Some(field) = placeholder.strip_prefix("ctx_") {
        return format!("$ctx.{field}");
    }
    format!("{placeholder} (engine)")
}
