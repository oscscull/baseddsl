//! Name helpers: the `$ref` builder and the snake_case -> UpperCamel schema/input naming.

use super::*;

/// A `$ref` to a named component schema.
pub(crate) fn schema_ref(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

/// The `components.schemas` name of a callable's input body (`create_order` -> `CreateOrderInput`).
pub(crate) fn input_name(name: &str) -> String {
    format!("{}Input", pascal(name))
}

/// snake_case / lower name -> UpperCamel (`order_by_id` -> `OrderById`). Already
/// UpperCamel shape/model names pass through unchanged. (Same rule as the client.)
pub(crate) fn pascal(name: &str) -> String {
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
