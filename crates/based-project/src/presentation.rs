//! Format compiler diagnostics with source locations for non-terminal callers.
use crate::Sources;
use based_diagnostics::Diagnostic;
use std::fmt;

pub(super) fn write_diagnostic(
    f: &mut fmt::Formatter<'_>,
    diagnostic: &Diagnostic,
    sources: &Sources,
) -> fmt::Result {
    if let Some(span) = diagnostic.span {
        if let Some((path, text)) = sources.get(span.file.0 as usize) {
            let before = &text.as_bytes()[..(span.start as usize).min(text.len())];
            let line = before.iter().filter(|&&byte| byte == b'\n').count() + 1;
            let column = before
                .iter()
                .rposition(|&byte| byte == b'\n')
                .map_or(before.len() + 1, |index| before.len() - index);
            write!(f, "{}:{line}:{column}: ", path.display())?;
        }
    }
    writeln!(f, "{}: {}", diagnostic.code, diagnostic.message)?;
    for note in &diagnostic.notes {
        writeln!(f, "  note: {note}")?;
    }
    Ok(())
}
