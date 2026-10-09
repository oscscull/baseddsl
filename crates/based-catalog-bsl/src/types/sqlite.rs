//! SQLite affinity does not authorize stronger application types or NUMERIC decimal.
use based_catalog::{NativeType, TypeFamily};
pub(super) fn map(native: &NativeType, declaration: &str) -> Option<(String, bool)> {
    let (mapped, canonical) = match (native.family, super::base(declaration)) {
        (
            TypeFamily::Integer,
            "integer" | "int" | "bigint" | "smallint" | "tinyint" | "int2" | "int8",
        ) => ("int", "integer"),
        (TypeFamily::Real, "real" | "float" | "double") => ("float", "real"),
        (TypeFamily::Text, "text" | "varchar" | "char" | "clob" | "nvarchar" | "nchar") => {
            ("text", "text")
        }
        (TypeFamily::Binary, "blob") => ("bytes", "blob"),
        _ => return None,
    };
    Some((mapped.into(), declaration == canonical))
}
