//! Split an `up.mig` into its structural residue, the `raw(<dialect>)` escapes, and the typed
//! data steps, each non-structural item anchored after the structural lines above it.

use super::super::diff::Step;
use crate::Dialect;

/// The first line of an in-order `up.mig`: every line, `raw` included, runs where it is
/// written. Without it a migration keeps the legacy order (all `raw` after the structure).
pub const FORMAT_MARKER: &str = "format 2";

pub(crate) struct Scanned<'a> {
    pub in_order: bool,
    /// Every line that is not the marker, a `raw` escape, or a data step.
    pub structural: Vec<&'a str>,
    pub items: Vec<Item>,
    /// A `raw(…)` region was seen (even one with an unknown dialect token).
    pub saw_raw: bool,
}

pub(crate) struct Item {
    /// 1-based line the item starts on.
    pub line: usize,
    /// How many canonical structural lines precede it.
    pub after: usize,
    pub kind: ItemKind,
}

pub(crate) enum ItemKind {
    Raw(Box<Step>),
    Data(String),
}

pub(crate) fn scan(up_text: &str) -> Scanned<'_> {
    let lines: Vec<&str> = up_text.lines().collect();
    let mut out = Scanned {
        in_order: false,
        structural: Vec::new(),
        items: Vec::new(),
        saw_raw: false,
    };
    let mut canonical = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed == FORMAT_MARKER {
            out.in_order = true;
            i += 1;
            continue;
        }
        if let Some((tok, opener)) = raw_line_opener(line) {
            if let Some((sql, next)) = raw_body(&lines, i, opener) {
                out.saw_raw = true;
                if let Some(step) = raw_step(tok, sql) {
                    out.items.push(Item {
                        line: i + 1,
                        after: canonical,
                        kind: ItemKind::Raw(Box::new(step)),
                    });
                }
                i = next;
                continue;
            }
        }
        if is_data_opener(trimmed) {
            let next = data_extent(&lines, i);
            out.items.push(Item {
                line: i + 1,
                after: canonical,
                kind: ItemKind::Data(lines[i..next].join("\n")),
            });
            i = next;
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            canonical += 1;
        }
        out.structural.push(line);
        i += 1;
    }
    out
}

/// If `line` opens a `raw(<dialect>)` step, its dialect token and the text after the opening
/// backtick.
fn raw_line_opener(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim().strip_prefix("raw(")?;
    let close = rest.find(')')?;
    let after = &rest[close + 1..];
    let open_q = after.find('`')?;
    Some((rest[..close].trim(), &after[open_q + 1..]))
}

/// The SQL of a raw step opened on line `i` and the index of the line after it: a closing
/// backtick later on the same line, or a block closed by a lone backtick line. `None` for an
/// unterminated block, which stays structural so the drift check surfaces it.
fn raw_body(lines: &[&str], i: usize, opener: &str) -> Option<(String, usize)> {
    if let Some(close) = opener.rfind('`') {
        return Some((opener[..close].trim().to_string(), i + 1));
    }
    let mut body = opener.trim().to_string();
    for (j, l) in lines.iter().enumerate().skip(i + 1) {
        if l.trim() == "`" {
            return Some((body.trim().to_string(), j + 1));
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(l);
    }
    None
}

/// A raw step for a known dialect token with a non-empty body (an unknown token can't
/// masquerade as a dialect).
fn raw_step(tok: &str, sql: String) -> Option<Step> {
    if sql.is_empty() {
        return None;
    }
    let dialect = match tok {
        "sqlite" => Dialect::Sqlite,
        "mysql" => Dialect::MySql,
        "mariadb" => Dialect::MariaDb,
        "postgres" | "postgresql" => Dialect::Postgres,
        _ => return None,
    };
    Some(Step::Raw { dialect, sql })
}

/// Does this line open a data step? `create table <name> {` is the structural create.
fn is_data_opener(trimmed: &str) -> bool {
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    match words.first().copied() {
        Some("update" | "delete") => words.len() > 1,
        Some("create") => {
            let structural = words.get(1) == Some(&"table")
                && words
                    .get(2)
                    .is_some_and(|w| !w.starts_with('{') && !w.starts_with('['));
            words.len() > 1 && !structural
        }
        _ => false,
    }
}

/// The index one past the last line of the data step starting at `start`: brackets balance,
/// and the next line does not continue it with `on conflict` or `chunk`.
fn data_extent(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i64;
    let mut j = start;
    while j < lines.len() {
        depth += bracket_delta(lines[j]);
        j += 1;
        if depth <= 0 && !continues(lines.get(j).copied()) {
            break;
        }
    }
    j
}

fn continues(next: Option<&str>) -> bool {
    next.map(str::trim_start)
        .is_some_and(|l| l.starts_with("on conflict") || l.starts_with("chunk "))
}

/// Net opening brackets on a line, ignoring string literals and `#` comments.
fn bracket_delta(line: &str) -> i64 {
    let mut depth = 0;
    let mut in_str = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_str {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '#' => break,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}
