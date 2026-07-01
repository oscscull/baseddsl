//! Terminal rendering of diagnostics.
//!
//! A compact `rustc`-style frame: header line, source location, the offending
//! line, and a caret underline. Kept dependency-free and deterministic so it is
//! easy to snapshot in tests and read in review.

use based_diagnostics::{Diagnostic, Severity};
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
