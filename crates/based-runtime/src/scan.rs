//! Named → positional placeholder translation.
//!
//! Codegen emits legible `:name` placeholders; the driver binds positional placeholders.
//! The runtime is the layer that translates, kept here so the generated SQL stays
//! readable. The positional spelling is dialect-specific: MySQL/MariaDB and SQLite bind an
//! anonymous `?`, Postgres binds ordinal `$1, $2, …`. So the scan takes the [`Dialect`] and
//! emits the right form — the rest of the rewrite is identical.
//!
//! The scan is quote-aware: a `:name` inside a `'...'` / `"..."` / `` `...` `` literal is
//! text, not a placeholder (a user can write `where (status = "a:b")`, and a raw block can
//! contain a time literal like `'12:30:00'`). A `::` — Postgres's cast operator, e.g.
//! `x::text` — is skipped whole so the second `:` never starts a spurious placeholder.
//!
//! The occurrence order comes from the SQL; the value for each name is resolved by the
//! caller (`plan`). So the runtime never maintains a parallel bind manifest — the SQL is
//! the one source of the bind surface.

use based_codegen::Dialect;

/// How one resolved placeholder fills the positional query: a single bind, or a
/// variable-length list expanded into one placeholder per element (`col IN (?, ?, …)`) —
/// the mechanism behind `col in $arr` for an array-typed param. An empty list fills its
/// placeholder with a literal `NULL` (`col IN (NULL)` matches nothing).
pub enum Bound<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> Bound<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(v) => vec![v],
            Self::Many(vs) => vs,
        }
    }
}

/// Rewrite `:name` placeholders to the dialect's positional form (`?` for
/// MySQL/MariaDB/SQLite, `$n` for Postgres), calling `resolve` for each in appearance
/// order to collect the bound values. A placeholder may resolve to a [`Bound::Many`] list,
/// which expands to one positional marker per element (variable-length `IN (…)`). `resolve`
/// returns `None` for an unknown placeholder; the scan then returns `Err(name)` so the
/// caller reports it.
///
/// A repeat `{:name AND body}` / `{:name OR body}` expands to one copy of `body` per element
/// of `:name`'s list, joined by the operator and parenthesized, each copy's `:name` bound to
/// its own element. An empty list yields the operator's identity (`TRUE` / `FALSE`).
pub fn to_positional<T: Clone>(
    sql: &str,
    dialect: Dialect,
    resolve: impl FnMut(&str) -> Option<Bound<T>>,
) -> Result<(String, Vec<T>), String> {
    to_positional_from(sql, dialect, 0, resolve)
}

/// Like [`to_positional`], but Postgres ordinals start at `offset + 1` — for splicing a SQL
/// fragment (a bulk upsert's `ON CONFLICT … SET` tail) after `offset` binds already emitted
/// on the same statement, so the fragment's `$n` continues the running count. `?`-dialects
/// are offset-independent. The returned `Vec` holds only the fragment's own binds.
pub fn to_positional_from<T: Clone>(
    sql: &str,
    dialect: Dialect,
    offset: usize,
    mut resolve: impl FnMut(&str) -> Option<Bound<T>>,
) -> Result<(String, Vec<T>), String> {
    let mut scan = Scan {
        dialect,
        offset,
        out: Vec::with_capacity(sql.len()),
        params: Vec::new(),
    };
    scan.run(sql, &mut resolve)?;
    // `out` is the original UTF-8 bytes with ASCII placeholders spliced in at char
    // boundaries — always valid UTF-8.
    let out = String::from_utf8(scan.out).expect("scan preserves UTF-8");
    Ok((out, scan.params))
}

/// The positional output under construction.
struct Scan<T> {
    dialect: Dialect,
    offset: usize,
    // Built as bytes so verbatim SQL text is copied intact: a multi-byte UTF-8 character
    // (a non-ASCII literal in a raw block, say) must survive byte-for-byte. Every delimiter
    // the scan tests for is ASCII, and UTF-8 never puts an ASCII byte inside a multi-byte
    // sequence, so the byte-level quote/placeholder detection is unaffected.
    out: Vec<u8>,
    params: Vec<T>,
}

impl<T: Clone> Scan<T> {
    fn run(
        &mut self,
        sql: &str,
        resolve: &mut dyn FnMut(&str) -> Option<Bound<T>>,
    ) -> Result<(), String> {
        let bytes = sql.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            // Inside a quoted literal: copy verbatim to the matching close quote.
            if is_quote(c) {
                let end = quoted_end(bytes, i);
                self.out.extend_from_slice(&bytes[i..end]);
                i = end;
                continue;
            }
            // `::` is not one of our placeholders (it is Postgres's cast operator) — copy
            // both colons and move on so the second `:` cannot start a spurious placeholder.
            if c == b':' && bytes.get(i + 1) == Some(&b':') {
                self.out.extend_from_slice(b"::");
                i += 2;
                continue;
            }
            if c == b'{' {
                if let Some(rep) = Repeat::parse(sql, i) {
                    self.repeat(&rep, resolve)?;
                    i = rep.end;
                    continue;
                }
            }
            if let Some(end) = placeholder_end(bytes, i) {
                let name = &sql[i + 1..end];
                let bound = resolve(name).ok_or_else(|| name.to_string())?;
                self.binds(bound.into_vec());
                i = end;
                continue;
            }
            self.out.push(c);
            i += 1;
        }
        Ok(())
    }

    /// One marker per value, comma-separated; none is a literal `NULL`.
    fn binds(&mut self, values: Vec<T>) {
        if values.is_empty() {
            self.out.extend_from_slice(b"NULL");
        }
        for (k, v) in values.into_iter().enumerate() {
            if k > 0 {
                self.out.extend_from_slice(b", ");
            }
            self.params.push(v);
            // Anonymous `?` or ordinal `$n` (1-based) per dialect. The ordinal is the running
            // parameter count, so it matches the bind order.
            match self.dialect {
                Dialect::Postgres => {
                    let n = self.offset + self.params.len();
                    self.out.extend_from_slice(format!("${n}").as_bytes());
                }
                _ => self.out.push(b'?'),
            }
        }
    }

    fn repeat(
        &mut self,
        rep: &Repeat<'_>,
        resolve: &mut dyn FnMut(&str) -> Option<Bound<T>>,
    ) -> Result<(), String> {
        let items = resolve(rep.name)
            .ok_or_else(|| rep.name.to_string())?
            .into_vec();
        if items.is_empty() {
            let identity: &[u8] = if rep.op == "AND" { b"TRUE" } else { b"FALSE" };
            self.out.extend_from_slice(identity);
            return Ok(());
        }
        self.out.push(b'(');
        for (k, item) in items.into_iter().enumerate() {
            if k > 0 {
                self.out
                    .extend_from_slice(format!(" {} ", rep.op).as_bytes());
            }
            let mut each = |name: &str| {
                if name == rep.name {
                    Some(Bound::One(item.clone()))
                } else {
                    resolve(name)
                }
            };
            self.run(rep.body, &mut each)?;
        }
        self.out.push(b')');
        Ok(())
    }
}

/// A `{:name OP body}` repeat starting at a `{`: the list placeholder, the joining operator,
/// the body, and the offset just past the closing `}`.
struct Repeat<'a> {
    name: &'a str,
    op: &'static str,
    body: &'a str,
    end: usize,
}

impl<'a> Repeat<'a> {
    fn parse(sql: &'a str, open: usize) -> Option<Self> {
        let bytes = sql.as_bytes();
        let name_end = placeholder_end(bytes, open + 1)?;
        let rest = &sql[name_end..];
        let op = ["AND", "OR"]
            .into_iter()
            .find(|op| rest.starts_with(&format!(" {op} ")))?;
        let body_start = name_end + op.len() + 2;
        let close = closing_brace(bytes, body_start)?;
        Some(Self {
            name: &sql[open + 2..name_end],
            op,
            body: &sql[body_start..close],
            end: close + 1,
        })
    }
}

/// The `}` closing a repeat body, skipping quoted literals and nested braces.
fn closing_brace(bytes: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            c if is_quote(c) => {
                i = quoted_end(bytes, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The end of a `:name` placeholder starting at `at`, if one starts there.
fn placeholder_end(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b':') || !bytes.get(at + 1).copied().is_some_and(is_ident_start) {
        return None;
    }
    let mut j = at + 1;
    while j < bytes.len() && is_ident_char(bytes[j]) {
        j += 1;
    }
    Some(j)
}

fn is_quote(b: u8) -> bool {
    b == b'\'' || b == b'"' || b == b'`'
}

/// Just past the quote closing the literal opened at `open`. SQL escapes a quote by
/// doubling it (`''`), which this handles naturally — the doubled close is seen as
/// close-then-reopen, leaving the scan still "inside".
fn quoted_end(bytes: &[u8], open: usize) -> usize {
    let q = bytes[open];
    let mut i = open + 1;
    while i < bytes.len() {
        i += 1;
        if bytes[i - 1] == q {
            break;
        }
    }
    i
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
        to_positional(sql, Dialect::MariaDb, |n| Some(Bound::One(n.to_string()))).unwrap()
    }

    fn go_pg(sql: &str) -> (String, Vec<String>) {
        to_positional(sql, Dialect::Postgres, |n| Some(Bound::One(n.to_string()))).unwrap()
    }

    #[test]
    fn basic_placeholders_in_order() {
        let (sql, ps) = go("WHERE a = :org AND b > :since");
        assert_eq!(sql, "WHERE a = ? AND b > ?");
        assert_eq!(ps, vec!["org", "since"]);
    }

    #[test]
    fn postgres_binds_ordinal_dollar_n() {
        // Postgres uses `$1, $2, …` in bind order — the one dialect coupling.
        let (sql, ps) = go_pg("WHERE a = :org AND b > :since AND c = :org");
        assert_eq!(sql, "WHERE a = $1 AND b > $2 AND c = $3");
        assert_eq!(ps, vec!["org", "since", "org"]);
    }

    #[test]
    fn postgres_cast_operator_untouched() {
        // `::text` is a Postgres cast, not a placeholder; only `:p` binds ($1).
        let (sql, ps) = go_pg("SELECT x::text WHERE p = :p");
        assert_eq!(sql, "SELECT x::text WHERE p = $1");
        assert_eq!(ps, vec!["p"]);
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
    fn non_ascii_literal_survives_verbatim() {
        // A multi-byte UTF-8 literal (a raw block can carry one) must be copied
        // byte-for-byte, not reinterpreted per-byte into mojibake.
        let (sql, ps) = go("WHERE name = 'café' AND emoji = '🎉' AND x = :p");
        assert_eq!(sql, "WHERE name = 'café' AND emoji = '🎉' AND x = ?");
        assert_eq!(ps, vec!["p"]);
    }

    #[test]
    fn unknown_placeholder_errors() {
        let err = to_positional::<String>("a = :nope", Dialect::MariaDb, |_| None).unwrap_err();
        assert_eq!(err, "nope");
    }

    #[test]
    fn list_param_expands_to_one_marker_per_element() {
        // `col in $arr` keeps a single `:arr` placeholder; a `Many` bound expands it to one
        // positional marker per element, in order, alongside the surrounding scalars.
        let (sql, ps) =
            to_positional(
                "WHERE org = :org AND c IN (:arr)",
                Dialect::MariaDb,
                |n| match n {
                    "arr" => Some(Bound::Many(vec!["R".to_string(), "U".to_string()])),
                    other => Some(Bound::One(other.to_string())),
                },
            )
            .unwrap();
        assert_eq!(sql, "WHERE org = ? AND c IN (?, ?)");
        assert_eq!(ps, vec!["org", "R", "U"]);
    }

    #[test]
    fn list_param_expands_ordinals_on_postgres() {
        // Postgres ordinals keep counting across the expanded elements and the trailing scalar.
        let (sql, ps) = to_positional("c IN (:arr) AND d = :d", Dialect::Postgres, |n| match n {
            "arr" => Some(Bound::Many(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ])),
            other => Some(Bound::One(other.to_string())),
        })
        .unwrap();
        assert_eq!(sql, "c IN ($1, $2, $3) AND d = $4");
        assert_eq!(ps, vec!["a", "b", "c", "d"]);
    }

    fn many(sql: &str, dialect: Dialect, list: &[&str]) -> (String, Vec<String>) {
        let list: Vec<String> = list.iter().map(ToString::to_string).collect();
        to_positional(sql, dialect, |n| match n {
            "arr" => Some(Bound::Many(list.clone())),
            other => Some(Bound::One(other.to_string())),
        })
        .unwrap()
    }

    #[test]
    fn empty_list_is_a_literal_null() {
        let (sql, ps) = many("c IN (:arr) AND d = :d", Dialect::Postgres, &[]);
        assert_eq!(sql, "c IN (NULL) AND d = $1");
        assert_eq!(ps, vec!["d"]);
    }

    #[test]
    fn repeat_copies_the_body_per_element() {
        let (sql, ps) = many(
            "a = :a AND {:arr OR `t`.`c` LIKE :arr} IS TRUE AND d = :d",
            Dialect::Sqlite,
            &["x", "y", "z"],
        );
        assert_eq!(
            sql,
            "a = ? AND (`t`.`c` LIKE ? OR `t`.`c` LIKE ? OR `t`.`c` LIKE ?) IS TRUE AND d = ?"
        );
        assert_eq!(ps, vec!["a", "x", "y", "z", "d"]);
    }

    #[test]
    fn repeat_continues_postgres_ordinals_and_resolves_other_names() {
        let (sql, ps) = many(
            "{:arr AND c LIKE :arr || :sfx} AND d = :d",
            Dialect::Postgres,
            &["x", "y"],
        );
        assert_eq!(sql, "(c LIKE $1 || $2 AND c LIKE $3 || $4) AND d = $5");
        assert_eq!(ps, vec!["x", "sfx", "y", "sfx", "d"]);
    }

    #[test]
    fn empty_repeat_is_the_operator_identity() {
        assert_eq!(
            many("{:arr AND c LIKE :arr}", Dialect::Sqlite, &[]).0,
            "TRUE"
        );
        assert_eq!(
            many("{:arr OR c LIKE :arr}", Dialect::Sqlite, &[]).0,
            "FALSE"
        );
    }

    #[test]
    fn repeat_body_skips_quoted_braces() {
        let (sql, _) = many("{:arr OR \"a}b\" LIKE :arr}", Dialect::Sqlite, &["x"]);
        assert_eq!(sql, "(\"a}b\" LIKE ?)");
    }

    #[test]
    fn a_brace_that_is_not_a_repeat_is_text() {
        let (sql, ps) = go("SELECT {fn now()}, {:x} = :y");
        assert_eq!(sql, "SELECT {fn now()}, {?} = ?");
        assert_eq!(ps, vec!["x", "y"]);
    }
}
