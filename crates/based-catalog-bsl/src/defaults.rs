//! Parse a deliberately closed native literal set; unknown SQL is never executed.
use super::{identifier::quote, ImportReport};
use based_catalog::{CatalogCode, CatalogDialect, Column, TableId};

pub(crate) fn render(
    column: &Column,
    table: &TableId,
    dialect: CatalogDialect,
    ty: &str,
    report: &mut ImportReport,
) -> Option<String> {
    let source = column.default.as_deref()?;
    let source = source.trim();
    let source = match dialect {
        CatalogDialect::Sqlite => strip_parentheses(source),
        _ => source,
    };
    let source = match (dialect, ty) {
        (CatalogDialect::Postgres, "text") => source
            .strip_suffix("::text")
            .or_else(|| source.strip_suffix("::character varying"))
            .unwrap_or(source),
        _ => source,
    };
    let value = literal(source, ty, dialect);
    if value.is_none() {
        report.error(table, Some(&column.name), CatalogCode::UnsupportedAttribute, "Default SQL is not a verified literal for this BSL type; expressions/casts/functions require manual representation");
    }
    value
}

fn strip_parentheses(mut source: &str) -> &str {
    while source.starts_with('(') && source.ends_with(')') {
        source = source[1..source.len() - 1].trim();
    }
    source
}

fn literal(source: &str, ty: &str, dialect: CatalogDialect) -> Option<String> {
    if source.eq_ignore_ascii_case("null") {
        return Some("null".into());
    }
    if ty == "bool" && matches!(source.to_ascii_lowercase().as_str(), "true" | "false") {
        return Some(source.to_ascii_lowercase());
    }
    if ty == "int"
        && !source.is_empty()
        && source.bytes().all(|byte| byte.is_ascii_digit())
        && source.parse::<i64>().is_ok()
    {
        return Some(source.to_owned());
    }
    if (ty == "float" || ty.starts_with("decimal(")) && decimal(source) {
        return Some(source.to_owned());
    }
    if ty != "text" {
        return None;
    }
    let body = source.strip_prefix('\'')?.strip_suffix('\'')?;
    // MariaDB's SQL mode affects backslash interpretation; reject it instead of guessing.
    if body.contains('\\') && dialect != CatalogDialect::Sqlite {
        return None;
    }
    let mut chars = body.chars();
    let mut decoded = String::new();
    while let Some(character) = chars.next() {
        if character == '\'' && chars.next()? != '\'' {
            return None;
        }
        decoded.push(character);
    }
    Some(quote(&decoded))
}

fn decimal(source: &str) -> bool {
    let Some((whole, fraction)) = source.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
}
