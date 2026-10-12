use super::*;
use based_sema::EnumValue;

/// The TypeScript expression for a placeholder, given the fixture's `n`.
pub(super) fn placeholder_expr(field: &str, placeholder: &Placeholder) -> String {
    match placeholder {
        Placeholder::Text => format!("`{field}-${{n}}`"),
        Placeholder::Int | Placeholder::Float => "n".to_string(),
        Placeholder::Decimal => "String(n)".to_string(),
        Placeholder::Bool => "false".to_string(),
        Placeholder::Enum(EnumValue::Str(wire)) => format!("{wire:?}"),
        Placeholder::Enum(EnumValue::Int(value)) => value.to_string(),
        Placeholder::Timestamp => "timestamp(n)".to_string(),
        Placeholder::Date => "date(n)".to_string(),
        Placeholder::Time => "time(n)".to_string(),
        Placeholder::Json => "{}".to_string(),
        Placeholder::Bytes => "\"\"".to_string(),
        Placeholder::Uuid => "uuid(n)".to_string(),
        Placeholder::Ulid => "ulid(n)".to_string(),
        Placeholder::Empty => "[]".to_string(),
    }
}
