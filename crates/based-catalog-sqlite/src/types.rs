use based_catalog::{NativeType, SqliteAffinity, TypeFamily};

pub(crate) fn read(declaration: String, strict: bool) -> NativeType {
    let uppercase = declaration.to_ascii_uppercase();
    let affinity = match (strict, uppercase.as_str()) {
        (true, "ANY") => SqliteAffinity::Blob,
        _ => affinity(&uppercase),
    };
    // Affinity is not evidence of UUID/boolean/date/decimal application semantics.
    let family = match (uppercase.as_str(), affinity) {
        ("" | "ANY", _) => TypeFamily::Other,
        (_, SqliteAffinity::Integer) => TypeFamily::Integer,
        (_, SqliteAffinity::Text) => TypeFamily::Text,
        (_, SqliteAffinity::Real) => TypeFamily::Real,
        (_, SqliteAffinity::Blob) => TypeFamily::Binary,
        (_, SqliteAffinity::Numeric) => TypeFamily::Other,
    };
    let mut native = NativeType::declared(declaration, family);
    native.sqlite_affinity = Some(affinity);
    native
}

fn affinity(declaration: &str) -> SqliteAffinity {
    if declaration.contains("INT") {
        return SqliteAffinity::Integer;
    }
    if ["CHAR", "CLOB", "TEXT"]
        .iter()
        .any(|token| declaration.contains(token))
    {
        return SqliteAffinity::Text;
    }
    if declaration.is_empty() || declaration.contains("BLOB") {
        return SqliteAffinity::Blob;
    }
    if ["REAL", "FLOA", "DOUB"]
        .iter()
        .any(|token| declaration.contains(token))
    {
        return SqliteAffinity::Real;
    }
    SqliteAffinity::Numeric
}
