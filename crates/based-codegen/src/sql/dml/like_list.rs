//! Lower `~ all` / `~ any` — LIKE against a runtime-sized pattern list.
//!
//! The predicate is always true or false — never NULL — so `not (…)` is its exact complement.
//! SQLite repeats the `LIKE` once per pattern (the runtime expands a `{:p OP …}` repeat); the
//! other dialects keep the single `:p` placeholder, which the runtime expands to one bind per
//! element (an empty list to `NULL`), and match the column against those binds as rows inside
//! `[NOT] EXISTS`.

use super::*;

#[derive(Clone, Copy)]
pub(crate) enum Quantifier {
    All,
    Any,
}

/// Some pattern the column matches (`any`; an empty list is false), or no pattern it fails
/// to match (`all`; an empty list is true, a NULL column false).
pub(crate) fn like_list(dialect: Dialect, lhs: &str, q: Quantifier, list: &str) -> String {
    if dialect == Dialect::Sqlite {
        return inline_likes(lhs, q, list);
    }
    let PatternRows {
        from,
        present,
        pattern,
    } = pattern_rows(dialect, list);
    match q {
        Quantifier::Any => {
            format!("EXISTS (SELECT 1 FROM {from} WHERE {present} AND {lhs} LIKE {pattern})")
        }
        Quantifier::All => format!(
            "NOT EXISTS (SELECT 1 FROM {from} WHERE {present} AND ({lhs} LIKE {pattern}) IS NOT TRUE)"
        ),
    }
}

/// One `LIKE` per pattern, joined by `OR` (`any`) or `AND` (`all`). A correlated row source
/// would be rebuilt for every candidate row; SQLite cannot hoist it.
fn inline_likes(lhs: &str, q: Quantifier, list: &str) -> String {
    let op = match q {
        Quantifier::All => "AND",
        Quantifier::Any => "OR",
    };
    format!("{{{list} {op} {lhs} LIKE {list}}} IS TRUE")
}

/// The expanded binds as rows: the row source, the test that a row is a pattern (not the
/// empty list's `NULL`), and the pattern itself.
struct PatternRows {
    from: String,
    present: &'static str,
    pattern: &'static str,
}

fn pattern_rows(dialect: Dialect, list: &str) -> PatternRows {
    match dialect {
        Dialect::Sqlite => unreachable!("SQLite inlines its patterns"),
        Dialect::Postgres => PatternRows {
            from: format!("unnest(ARRAY[{list}]::text[]) AS __pattern(value)"),
            present: "__pattern.value IS NOT NULL",
            pattern: "__pattern.value",
        },
        // A JSON column unquoted is a coercible string, so LIKE takes the column's collation
        // (a TEXT column would carry its own and clash). MariaDB reads the `NULL` element as
        // JSON `null`, MySQL as SQL NULL; neither is a string.
        Dialect::MariaDb | Dialect::MySql => PatternRows {
            from: format!(
                "JSON_TABLE(JSON_ARRAY({list}), '$[*]' COLUMNS (value JSON PATH '$')) AS __pattern"
            ),
            present: "JSON_TYPE(__pattern.value) = 'STRING'",
            pattern: "JSON_UNQUOTE(__pattern.value)",
        },
    }
}
