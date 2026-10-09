//! MariaDB native UUID/JSON and duration TIME are not guessed into narrower types.
use based_catalog::{NativeType, TypeFamily};
pub(super) fn map(native: &NativeType, declaration: &str) -> Option<(String, bool)> {
    let (mapped, canonical) = match (native.family, super::base(declaration)) {
        (
            TypeFamily::Integer,
            "bigint" | "int" | "integer" | "smallint" | "mediumint" | "tinyint",
        ) => ("int", "bigint"),
        (TypeFamily::Real, "double" | "real" | "float") => ("float", "double"),
        (
            TypeFamily::Text,
            "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext",
        ) => ("text", "varchar(255)"),
        (
            TypeFamily::Binary,
            "blob" | "tinyblob" | "mediumblob" | "longblob" | "binary" | "varbinary",
        ) => ("bytes", "blob"),
        (TypeFamily::Json, "json") => ("json", "json"),
        (TypeFamily::Date, "date") => ("date", "date"),
        (TypeFamily::Timestamp, "datetime") => ("timestamp", "datetime"),
        (TypeFamily::Decimal, "decimal") => return super::decimal::map(native),
        _ => return None,
    };
    Some((
        mapped.into(),
        declaration == canonical || (mapped == "int" && declaration == "bigint(20)"),
    ))
}
