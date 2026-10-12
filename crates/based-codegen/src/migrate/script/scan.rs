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
    let entries = std::iter::successors(
        (!lines.is_empty()).then(|| entry(&lines, 0)),
        |(_, _, next)| (*next < lines.len()).then(|| entry(&lines, *next)),
    );
    for (start, kind, _) in entries {
        let item = |kind| Item {
            line: start + 1,
            after: canonical,
            kind,
        };
        match kind {
            Entry::Marker => out.in_order = true,
            Entry::Raw(step) => {
                out.saw_raw = true;
                out.items.extend(step.map(|s| item(ItemKind::Raw(s))));
            }
            Entry::Data(text) => out.items.push(item(ItemKind::Data(text))),
            Entry::Structural(line) => {
                canonical += usize::from(is_canonical(line));
                out.structural.push(line);
            }
        }
    }
    out
}

/// What one script entry is, by its first line.
enum Entry<'a> {
    Marker,
    /// A terminated `raw(…)` region; `None` for an unknown dialect or an empty body.
    Raw(Option<Box<Step>>),
    Data(String),
    Structural(&'a str),
}

/// The entry starting at line `i`: its start, kind, and the index of the line after it.
fn entry<'a>(lines: &[&'a str], i: usize) -> (usize, Entry<'a>, usize) {
    let line = lines[i];
    let trimmed = line.trim();
    if trimmed == FORMAT_MARKER {
        return (i, Entry::Marker, i + 1);
    }
    let raw = raw_line_opener(line).and_then(|(tok, opener)| {
        raw_body(lines, i, opener).map(|(sql, next)| (raw_step(tok, sql).map(Box::new), next))
    });
    if let Some((step, next)) = raw {
        return (i, Entry::Raw(step), next);
    }
    if is_data_opener(trimmed) {
        let next = data_extent(lines, i);
        return (i, Entry::Data(lines[i..next].join("\n")), next);
    }
    (i, Entry::Structural(line), i + 1)
}

/// A line that counts toward the structural text: neither blank nor a `#` comment.
pub(crate) fn is_canonical(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.starts_with('#')
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
    let end = i + 1 + lines[i + 1..].iter().position(|l| l.trim() == "`")?;
    let body = std::iter::once(opener.trim())
        .chain(lines[i + 1..end].iter().copied())
        .collect::<Vec<_>>()
        .join("\n");
    Some((body.trim().to_string(), end + 1))
}

/// A raw step for a known dialect token with a non-empty body (an unknown token can't
/// masquerade as a dialect).
fn raw_step(tok: &str, sql: String) -> Option<Step> {
    let dialect = match tok {
        "sqlite" => Dialect::Sqlite,
        "mysql" => Dialect::MySql,
        "mariadb" => Dialect::MariaDb,
        "postgres" | "postgresql" => Dialect::Postgres,
        _ => return None,
    };
    (!sql.is_empty()).then_some(Step::Raw { dialect, sql })
}

/// Does this line open a data step? `create table <name> {` is the structural create.
fn is_data_opener(trimmed: &str) -> bool {
    let mut words = trimmed.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("update" | "delete"), Some(_), _) => true,
        (Some("create"), Some("table"), Some(name)) => name.starts_with(['{', '[']),
        (Some("create"), Some(_), _) => true,
        _ => false,
    }
}

/// The index one past the last line of the data step starting at `start`: brackets balance,
/// and the next line does not continue it with `on conflict` or `chunk`.
fn data_extent(lines: &[&str], start: usize) -> usize {
    lines[start..]
        .iter()
        .scan(0i64, |depth, l| {
            *depth += bracket_delta(l);
            Some(*depth)
        })
        .zip(start + 1..)
        .find(|&(depth, next)| depth <= 0 && !continues(lines.get(next).copied()))
        .map_or(lines.len(), |(_, next)| next)
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
