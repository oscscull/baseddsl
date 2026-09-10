pub(super) fn input_name(name: &str) -> String {
    format!("{}Input", pascal(name))
}

pub(super) fn ctx_name(name: &str) -> String {
    format!("{}Ctx", pascal(name))
}

pub(super) fn route_const(name: &str) -> String {
    format!("{}_ROUTE", name.to_uppercase())
}

/// snake_case / lower name -> UpperCamel (`order_by_id` -> `OrderById`). Already
/// UpperCamel shape/model names pass through unchanged.
pub(super) fn pascal(name: &str) -> String {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut cs = s.chars();
            match cs.next() {
                Some(first) => first.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Escape a field/method name that collides with a Rust keyword (`type` ->
/// `r#type`). The DSL's identifier set is broader than Rust's reserved words.
pub(super) fn field_ident(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "type", "match", "move", "ref", "box", "fn", "let", "mut", "impl", "trait", "struct",
        "enum", "self", "crate", "super", "async", "await", "dyn", "loop", "where",
    ];
    if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}
