//! Small parse/quote helpers over snapshot text and column lists.

use super::*;

/// Split a canonical `raw({ a: "x, y", b: "z" })` map body on the commas that separate
/// entries — a comma inside a quoted literal stays part of its entry.
pub(crate) fn split_map_entries(map: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in map.chars() {
        match c {
            _ if esc => {
                cur.push(c);
                esc = false;
            }
            '\\' if in_str => {
                cur.push(c);
                esc = true;
            }
            '"' => {
                in_str = !in_str;
                cur.push(c);
            }
            ',' if !in_str => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Unescape a snapshot string literal (`"…"` with `\"`/`\\` escapes).
pub(crate) fn unquote_snap(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) else {
        return s.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Quote a physical column list for the dialect, comma-joined.
pub(crate) fn quote_cols(dialect: Dialect, cols: &[String]) -> String {
    cols.iter()
        .map(|c| dialect.quote(c))
        .collect::<Vec<_>>()
        .join(", ")
}
