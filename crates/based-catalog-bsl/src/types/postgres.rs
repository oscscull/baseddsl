//! PostgreSQL mappings preserve timezone/JSON storage distinctions and numeric bounds.
use based_catalog::{NativeType, TypeFamily};
pub(super) fn map(native: &NativeType, declaration: &str) -> Option<(String, bool)> {
    let base = super::base(declaration);
    let (mapped, canonical) = match (native.family, base) {
        (TypeFamily::Integer, "bigint" | "integer" | "smallint") => ("int", "bigint"),
        (TypeFamily::Real, "double" | "real") => ("float", "double precision"),
        (TypeFamily::Boolean, "boolean") => ("bool", "boolean"),
        (TypeFamily::Text, "text" | "character") => ("text", "text"),
        (TypeFamily::Binary, "bytea") => ("bytes", "bytea"),
        (TypeFamily::Uuid, "uuid") => ("uuid", "uuid"),
        (TypeFamily::Json, "jsonb") => ("json", "jsonb"),
        (TypeFamily::Date, "date") => ("date", "date"),
        (TypeFamily::Timestamp, "timestamp") if native.timezone == Some(true) => {
            ("timestamp", "timestamp with time zone")
        }
        (TypeFamily::Time, "time") if native.timezone == Some(false) => {
            ("time", "time without time zone")
        }
        (TypeFamily::Decimal, "numeric" | "decimal") => return super::decimal::map(native),
        _ => return None,
    };
    Some((mapped.into(), declaration == canonical))
}
