//! Named → positional placeholder translation.
//!
//! Codegen emits legible `:name` placeholders (D11); MariaDB binds positional `?`.
//! The runtime is the layer that translates — a driver concern, kept here so the
//! generated SQL stays readable. The scan is **quote-aware**: a `:name` inside a
//! `'...'` / `"..."` / `` `...` `` literal is text, not a placeholder (a user can
//! write `where (status = "a:b")`, and a raw block can contain a time literal like
//! `'12:30:00'`). A `::` (not a placeholder in our emitted SQL) is skipped whole.
//!
//! The occurrence *order* comes from the SQL; the *value* for each name is resolved
//! by the caller (`plan`), which knows every arg / `$ctx` / pagination value. So the
//! runtime never maintains a parallel bind manifest — the SQL is the one source of
//! the bind surface (principle 4).

/// Rewrite `:name` placeholders to positional `?`, calling `resolve` for each in
/// appearance order to collect the bound values. `resolve` returns `None` for an
/// unknown placeholder; the scan then returns `Err(name)` so the caller reports it.
pub fn to_positional<T>(
    sql: &str,
    mut resolve: impl FnMut(&str) -> Option<T>,
) -> Result<(String, Vec<T>), String> {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut params = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Inside a quoted literal: copy verbatim to the matching close quote.
        // SQL escapes a quote by doubling it (`''`), which this handles naturally —
        // the doubled close is seen as close-then-reopen, leaving us still "inside".
        if c == b'\'' || c == b'"' || c == b'`' {
            out.push(c as char);
            i += 1;
            while i < bytes.len() {
                out.push(bytes[i] as char);
                i += 1;
                if bytes[i - 1] == c {
                    break;
                }
            }
            continue;
        }
        // `::` is not one of our placeholders — copy both colons and move on so the
        // second `:` cannot start a spurious placeholder.
        if c == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':' {
            out.push_str("::");
            i += 2;
            continue;
        }
        // `:name` — an identifier placeholder.
        if c == b':' && i + 1 < bytes.len() && is_ident_start(bytes[i + 1]) {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && is_ident_char(bytes[j]) {
                j += 1;
            }
            let name = &sql[start..j];
            match resolve(name) {
                Some(v) => {
                    out.push('?');
                    params.push(v);
                    i = j;
                    continue;
                }
                None => return Err(name.to_string()),
            }
        }
        out.push(c as char);
        i += 1;
    }
    Ok((out, params))
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(sql: &str) -> (String, Vec<String>) {
        to_positional(sql, |n| Some(n.to_string())).unwrap()
    }

    #[test]
    fn basic_placeholders_in_order() {
        let (sql, ps) = go("WHERE a = :org AND b > :since");
        assert_eq!(sql, "WHERE a = ? AND b > ?");
        assert_eq!(ps, vec!["org", "since"]);
    }

    #[test]
    fn colon_inside_string_is_not_a_placeholder() {
        let (sql, ps) = go("WHERE status = 'a:b' AND x = :real");
        assert_eq!(sql, "WHERE status = 'a:b' AND x = ?");
        assert_eq!(ps, vec!["real"]);
    }

    #[test]
    fn time_literal_untouched() {
        let (sql, ps) = go("WHERE t = '12:30:00'");
        assert_eq!(sql, "WHERE t = '12:30:00'");
        assert!(ps.is_empty());
    }

    #[test]
    fn doubled_quote_escape_stays_inside() {
        // 'it''s :x' is one literal; :x must not be treated as a placeholder.
        let (sql, ps) = go("WHERE s = 'it''s :x' AND y = :z");
        assert_eq!(sql, "WHERE s = 'it''s :x' AND y = ?");
        assert_eq!(ps, vec!["z"]);
    }

    #[test]
    fn double_colon_skipped() {
        let (sql, ps) = go("SELECT x::text, :p");
        assert_eq!(sql, "SELECT x::text, ?");
        assert_eq!(ps, vec!["p"]);
    }

    #[test]
    fn unknown_placeholder_errors() {
        let err = to_positional::<String>("a = :nope", |_| None).unwrap_err();
        assert_eq!(err, "nope");
    }
}
