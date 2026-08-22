//! A per-dialect highlighter for the SQL inside a `raw`…`` block.
//!
//! The editor renders a `.bsl` file's TextMate grammar first; over it the server
//! paints semantic tokens for the embedded SQL, tokenized against the project's own
//! compile-target dialect (the manifest `dialect`) so the highlighting matches the SQL
//! the engine will actually emit — MySQL/MariaDB backtick quoting and `#` comments,
//! Postgres double-quoted identifiers and `$$`-delimited strings. This is highlighting,
//! not parsing: it classifies runs, it does not validate the SQL.

use based_codegen::Dialect;

/// A classified run of the raw-SQL interior. `start`/`len` are byte offsets **within**
/// the interior string passed to [`tokenize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlToken {
    pub start: usize,
    pub len: usize,
    pub kind: SqlTok,
}

/// The highlight class of a run. The order of [`TOKEN_TYPES`] fixes each kind's index
/// in the LSP legend; [`SqlTok::type_index`] is the single source of that mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlTok {
    Keyword,
    /// A string literal (`'…'`, or a Postgres `$$…$$` dollar-quoted body).
    Str,
    Number,
    Comment,
    /// A quoted identifier — Postgres `"col"` (MySQL/MariaDB/SQLite quote with backticks,
    /// which cannot occur inside a backtick-delimited `raw`…`` block).
    Ident,
    /// `${param}` — a bound parameter interpolation.
    Param,
    /// `{table}` / `{id}` — an engine-provided safe interpolation.
    Engine,
}

/// The LSP semantic-token-type names, in legend order. `SqlTok::type_index` indexes
/// into this; the server builds its `SemanticTokensLegend` from the same slice so the
/// two never drift.
pub const TOKEN_TYPES: [&str; 7] = [
    "keyword",   // Keyword
    "string",    // Str
    "number",    // Number
    "comment",   // Comment
    "variable",  // Ident
    "parameter", // Param
    "macro",     // Engine
];

impl SqlTok {
    /// This kind's index into [`TOKEN_TYPES`] / the LSP legend.
    pub fn type_index(self) -> u32 {
        match self {
            Self::Keyword => 0,
            Self::Str => 1,
            Self::Number => 2,
            Self::Comment => 3,
            Self::Ident => 4,
            Self::Param => 5,
            Self::Engine => 6,
        }
    }
}

/// Classify the interior of one `raw`…`` block for `dialect`. `src` is the text between
/// the backticks; returned offsets are relative to its start, in source order.
pub fn tokenize(src: &str, dialect: Dialect) -> Vec<SqlToken> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == b'$' && next == Some(b'{') {
            // `${param}` — a bound parameter, whole `${…}` run is one token.
            let end = find(b, i + 2, b'}').map_or(b.len(), |j| j + 1);
            out.push(tok(i, end, SqlTok::Param));
            i = end;
        } else if c == b'{' {
            // `{engine}` — safe engine interpolation.
            let end = find(b, i + 1, b'}').map_or(b.len(), |j| j + 1);
            out.push(tok(i, end, SqlTok::Engine));
            i = end;
        } else if (c == b'-' && next == Some(b'-')) || (c == b'#' && dialect.is_mysql_family()) {
            // A line comment: `--` in every dialect, `#` in the MySQL/MariaDB family.
            let end = line_end(b, i);
            out.push(tok(i, end, SqlTok::Comment));
            i = end;
        } else if c == b'/' && next == Some(b'*') {
            let end = find_seq(b, i + 2, b"*/").map_or(b.len(), |j| j + 2);
            out.push(tok(i, end, SqlTok::Comment));
            i = end;
        } else if c == b'\'' {
            let end = string_end(b, i, b'\'');
            out.push(tok(i, end, SqlTok::Str));
            i = end;
        } else if c == b'"' {
            let end = string_end(b, i, b'"');
            // Only Postgres reads `"…"` as an identifier; elsewhere it is a string.
            let kind = if matches!(dialect, Dialect::Postgres) {
                SqlTok::Ident
            } else {
                SqlTok::Str
            };
            out.push(tok(i, end, kind));
            i = end;
        } else if c == b'$' && matches!(dialect, Dialect::Postgres) {
            // A Postgres dollar-quoted string: `$$…$$` or `$tag$…$tag$`.
            if let Some((body_start, tag_end)) = dollar_tag(b, i) {
                let close =
                    find_seq(b, body_start, &b[i..tag_end]).map_or(b.len(), |j| j + (tag_end - i));
                out.push(tok(i, close, SqlTok::Str));
                i = close;
            } else {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            let end = number_end(b, i);
            out.push(tok(i, end, SqlTok::Number));
            i = end;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let end = word_end(b, i);
            if is_keyword(&src[i..end]) {
                out.push(tok(i, end, SqlTok::Keyword));
            }
            i = end;
        } else {
            // Advance one whole UTF-8 char so multibyte text never splits mid-codepoint.
            i += utf8_len(c);
        }
    }
    out
}

fn tok(start: usize, end: usize, kind: SqlTok) -> SqlToken {
    SqlToken {
        start,
        len: end - start,
        kind,
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn find(b: &[u8], from: usize, needle: u8) -> Option<usize> {
    (from..b.len()).find(|&i| b[i] == needle)
}

fn find_seq(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from > b.len() {
        return None;
    }
    (from..=b.len().saturating_sub(needle.len())).find(|&i| &b[i..i + needle.len()] == needle)
}

/// Byte offset just past the current line (the newline, or end of input).
fn line_end(b: &[u8], from: usize) -> usize {
    find(b, from, b'\n').unwrap_or(b.len())
}

/// End of a quoted literal opened at `open`, honoring the doubled-quote escape
/// (`''` / `""`). Returns the offset just past the closing quote (or end of input).
fn string_end(b: &[u8], open: usize, quote: u8) -> usize {
    let mut i = open + 1;
    while i < b.len() {
        if b[i] == quote {
            if b.get(i + 1) == Some(&quote) {
                i += 2; // doubled quote — an escaped literal quote, stays open
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

/// A number run: digits, an optional fractional part, and an optional exponent.
fn number_end(b: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    i
}

/// End of an identifier/keyword word: ASCII letters, digits, and `_`.
fn word_end(b: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    i
}

/// At a `$`, read a dollar-quote opening tag `$…$`. Returns `(body_start, tag_end)`
/// where the tag is `b[open..tag_end]`, or `None` if it is not a well-formed tag.
fn dollar_tag(b: &[u8], open: usize) -> Option<(usize, usize)> {
    let mut i = open + 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    if i < b.len() && b[i] == b'$' {
        Some((i + 1, i + 1))
    } else {
        None
    }
}

/// SQL keywords shared across the target dialects (matched case-insensitively). This
/// is a highlighting palette, not the full grammar of any one dialect.
fn is_keyword(word: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "select",
        "from",
        "where",
        "and",
        "or",
        "not",
        "in",
        "is",
        "null",
        "as",
        "join",
        "inner",
        "left",
        "right",
        "full",
        "outer",
        "cross",
        "on",
        "using",
        "group",
        "by",
        "having",
        "order",
        "asc",
        "desc",
        "limit",
        "offset",
        "distinct",
        "union",
        "all",
        "except",
        "intersect",
        "insert",
        "into",
        "values",
        "update",
        "set",
        "delete",
        "returning",
        "with",
        "recursive",
        "case",
        "when",
        "then",
        "else",
        "end",
        "cast",
        "coalesce",
        "nullif",
        "exists",
        "between",
        "like",
        "ilike",
        "escape",
        "true",
        "false",
        "count",
        "sum",
        "avg",
        "min",
        "max",
        "over",
        "partition",
        "window",
        "filter",
        "for",
        "no",
        "key",
        "share",
        "nowait",
        "skip",
        "locked",
        "lateral",
        "default",
        "current_timestamp",
        "current_date",
        "current_time",
        "now",
        "interval",
    ];
    KEYWORDS.iter().any(|k| word.eq_ignore_ascii_case(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slice each token back out of `src` and pair it with its kind — the readable form
    /// the assertions compare against.
    fn kinds(src: &str, dialect: Dialect) -> Vec<(SqlTok, &str)> {
        tokenize(src, dialect)
            .into_iter()
            .map(|t| (t.kind, &src[t.start..t.start + t.len]))
            .collect()
    }

    #[test]
    fn keywords_are_case_insensitive_and_bounded() {
        let got = kinds("SELECT name from user", Dialect::MariaDb);
        assert_eq!(
            got,
            vec![(SqlTok::Keyword, "SELECT"), (SqlTok::Keyword, "from"),]
        );
        // `format` is not a keyword even though it contains `for`.
        assert!(kinds("format", Dialect::MariaDb).is_empty());
    }

    #[test]
    fn param_interpolation_is_its_own_token() {
        let got = kinds("total >= ${min}", Dialect::MariaDb);
        assert_eq!(got, vec![(SqlTok::Param, "${min}")]);
        // A dotted path stays inside one param token.
        let got = kinds("${ctx.org}", Dialect::Postgres);
        assert_eq!(got, vec![(SqlTok::Param, "${ctx.org}")]);
    }

    #[test]
    fn engine_interpolation_is_distinct_from_param() {
        let got = kinds("from {table} where id = {id}", Dialect::MariaDb);
        assert_eq!(
            got,
            vec![
                (SqlTok::Keyword, "from"),
                (SqlTok::Engine, "{table}"),
                (SqlTok::Keyword, "where"),
                (SqlTok::Engine, "{id}"),
            ]
        );
    }

    #[test]
    fn single_quoted_strings_honor_doubled_quote() {
        let got = kinds("name = 'O''Brien' or x", Dialect::Sqlite);
        assert_eq!(
            got,
            vec![(SqlTok::Str, "'O''Brien'"), (SqlTok::Keyword, "or"),]
        );
    }

    #[test]
    fn numbers_include_decimal_and_exponent() {
        assert_eq!(
            kinds("1 2.5 3e10 4.2E-3", Dialect::Postgres),
            vec![
                (SqlTok::Number, "1"),
                (SqlTok::Number, "2.5"),
                (SqlTok::Number, "3e10"),
                (SqlTok::Number, "4.2E-3"),
            ]
        );
    }

    #[test]
    fn line_and_block_comments() {
        assert_eq!(
            kinds("a -- tail\nselect", Dialect::Postgres),
            vec![(SqlTok::Comment, "-- tail"), (SqlTok::Keyword, "select"),]
        );
        assert_eq!(
            kinds("/* a\nb */ from", Dialect::Postgres),
            vec![(SqlTok::Comment, "/* a\nb */"), (SqlTok::Keyword, "from"),]
        );
    }

    #[test]
    fn hash_comment_only_in_mysql_family() {
        assert_eq!(
            kinds("# note\nx", Dialect::MariaDb),
            vec![(SqlTok::Comment, "# note")]
        );
        // On Postgres `#` is not a comment lead-in — nothing is classified.
        assert!(kinds("# note\nx", Dialect::Postgres).is_empty());
    }

    #[test]
    fn double_quote_is_identifier_on_postgres_string_elsewhere() {
        assert_eq!(
            kinds("\"order\"", Dialect::Postgres),
            vec![(SqlTok::Ident, "\"order\"")]
        );
        assert_eq!(
            kinds("\"order\"", Dialect::MariaDb),
            vec![(SqlTok::Str, "\"order\"")]
        );
    }

    #[test]
    fn postgres_dollar_quoted_string() {
        assert_eq!(
            kinds("$$a 'b' c$$ from", Dialect::Postgres),
            vec![(SqlTok::Str, "$$a 'b' c$$"), (SqlTok::Keyword, "from"),]
        );
        // A tagged variant, and the tag is not mistaken for a param.
        assert_eq!(
            kinds("$tag$x$tag$", Dialect::Postgres),
            vec![(SqlTok::Str, "$tag$x$tag$")]
        );
        // `$$` is not special on MySQL/MariaDB.
        assert!(kinds("$$x$$", Dialect::MariaDb).is_empty());
    }

    #[test]
    fn param_beats_dollar_quote_on_postgres() {
        assert_eq!(
            kinds("${min}", Dialect::Postgres),
            vec![(SqlTok::Param, "${min}")]
        );
    }

    #[test]
    fn multibyte_text_between_tokens_does_not_panic() {
        // Non-ASCII literal text is skipped a whole codepoint at a time.
        let got = kinds("'café' from ünïcode", Dialect::Postgres);
        assert_eq!(
            got,
            vec![(SqlTok::Str, "'café'"), (SqlTok::Keyword, "from"),]
        );
    }

    #[test]
    fn unterminated_constructs_stop_at_end() {
        // No closing quote / brace — the token runs to the end rather than looping.
        assert_eq!(
            kinds("'oops", Dialect::Sqlite),
            vec![(SqlTok::Str, "'oops")]
        );
        assert_eq!(
            kinds("${min", Dialect::Sqlite),
            vec![(SqlTok::Param, "${min")]
        );
        assert_eq!(
            kinds("/* open", Dialect::Sqlite),
            vec![(SqlTok::Comment, "/* open")]
        );
    }
}
