//! Terminal rendering of diagnostics.
//!
//! A compact `rustc`-style frame: header line, source location, the offending
//! line, and a caret underline. Kept dependency-free and deterministic so it is
//! easy to snapshot in tests and read in review.

use based_diagnostics::{Diagnostic, Severity};
use based_facts::Fact;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Print each diagnostic to stderr. `sources` is indexed by `FileId`; a
/// diagnostic without a span (whole-project errors) prints just its header.
pub fn render(diags: &[Diagnostic], sources: &[(PathBuf, String)]) {
    for d in diags {
        let sev = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{sev}[{}]: {}", d.code, d.message);

        if let Some(span) = d.span {
            if let Some((path, src)) = sources.get(span.file.0 as usize) {
                let loc = locate(src, span.start as usize);
                eprintln!("  --> {}:{}:{}", path.display(), loc.line, loc.col);
                let gutter = loc.line.to_string();
                let pad = " ".repeat(gutter.len());
                eprintln!("{pad} |");
                eprintln!("{gutter} | {}", loc.text);
                let carets = ((span.end - span.start).max(1) as usize)
                    .min(loc.text.len().saturating_sub(loc.col - 1).max(1));
                eprintln!("{pad} | {}{}", " ".repeat(loc.col - 1), "^".repeat(carets));
            }
        }
        for note in &d.notes {
            eprintln!("  = note: {note}");
        }
        eprintln!();
    }
}

struct Loc<'a> {
    line: usize,
    col: usize,
    text: &'a str,
}

/// Resolve a byte offset to 1-based line/column and the text of that line.
fn locate(src: &str, offset: usize) -> Loc<'_> {
    let offset = offset.min(src.len());
    let mut line_start = 0;
    let mut line = 1;
    for (i, b) in src.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let line_end = src[line_start..]
        .find('\n')
        .map(|n| line_start + n)
        .unwrap_or(src.len());
    Loc {
        line,
        col: offset - line_start + 1,
        text: &src[line_start..line_end],
    }
}

/// `path:line:col` for a fact's anchor span, resolved through `sources`.
fn loc_of(fact: &Fact, sources: &[(PathBuf, String)]) -> (String, usize, usize) {
    match sources.get(fact.span.file.0 as usize) {
        Some((path, src)) => {
            let loc = locate(src, fact.span.start as usize);
            (path.display().to_string(), loc.line, loc.col)
        }
        None => ("<unknown>".to_string(), 0, 0),
    }
}

/// Human-readable derived-facts listing (the `based facts` default). Each fact is a
/// located header line plus an indented "why" note, echoing the diagnostic voice.
pub fn facts_text(facts: &[Fact], sources: &[(PathBuf, String)]) -> String {
    let mut out = String::new();
    for f in facts {
        let (path, line, col) = loc_of(f, sources);
        let _ = writeln!(out, "{path}:{line}:{col}  {}  {}", f.kind.tag(), f.label);
        let _ = writeln!(out, "  = {}", f.detail);
    }
    out
}

/// Machine-readable derived facts: a deterministic JSON array (no serde dependency
/// — the shape is tiny and stable, and hand-rolling keeps the CLI lean).
pub fn facts_json(facts: &[Fact], sources: &[(PathBuf, String)]) -> String {
    let mut out = String::from("[");
    for (i, f) in facts.iter().enumerate() {
        let (path, line, col) = loc_of(f, sources);
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "\n  {{\"file\": {}, \"line\": {line}, \"col\": {col}, \"kind\": {}, \"label\": {}, \"detail\": {}}}",
            json_str(&path),
            json_str(f.kind.tag()),
            json_str(&f.label),
            json_str(&f.detail),
        );
    }
    out.push_str(if facts.is_empty() { "]\n" } else { "\n]\n" });
    out
}

/// Minimal JSON string escaping (quotes, backslash, control chars).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
