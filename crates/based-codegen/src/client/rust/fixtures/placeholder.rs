use super::*;
use based_sema::EnumValue;

/// The Rust expression (a `serde_json::Value`) for a placeholder, given the fixture's `n`.
pub(super) fn placeholder_expr(field: &str, placeholder: &Placeholder) -> String {
    match placeholder {
        Placeholder::Text => format!("serde_json::Value::from(format!(\"{field}-{{n}}\"))"),
        Placeholder::Int => "serde_json::Value::from(n)".to_string(),
        Placeholder::Float => "serde_json::Value::from(n as f64)".to_string(),
        Placeholder::Decimal { whole_digits } => format!(
            "serde_json::Value::from((n % {}).to_string())",
            10u64.pow(*whole_digits)
        ),
        Placeholder::Bool => "serde_json::Value::Bool(false)".to_string(),
        Placeholder::Enum(EnumValue::Str(wire)) => format!("serde_json::Value::from({wire:?})"),
        Placeholder::Enum(EnumValue::Int(value)) => format!("serde_json::Value::from({value}_i64)"),
        Placeholder::Timestamp => "serde_json::Value::from(support::timestamp(n))".to_string(),
        Placeholder::Date => "serde_json::Value::from(support::date(n))".to_string(),
        Placeholder::Time => "serde_json::Value::from(support::time(n))".to_string(),
        Placeholder::Json => "serde_json::Value::Object(serde_json::Map::new())".to_string(),
        Placeholder::Bytes => "serde_json::Value::from(\"\")".to_string(),
        Placeholder::Uuid => "serde_json::Value::from(support::uuid(n))".to_string(),
        Placeholder::Ulid => "serde_json::Value::from(support::ulid(n))".to_string(),
        Placeholder::Empty => "serde_json::Value::Array(Vec::new())".to_string(),
    }
}
