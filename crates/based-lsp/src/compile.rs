//! Compiling an in-editor snapshot.
//!
//! The server runs the same front end as `based check` — discover the project's
//! `.bsl` set, overlay any unsaved editor buffers, parse + check — then keeps the
//! result (facts + diagnostics + a line index per file) so inlay-hint / hover /
//! diagnostic requests are served without recompiling. The `FileId` a span carries
//! is the index into `sources`, exactly as the CLI builds it.

use based_ast::{Decl, FileId};
use based_diagnostics::Diagnostic;
use based_facts::Fact;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Position;

/// A compiled view of the project the server answers requests from.
pub struct Snapshot {
    /// Sources indexed by `FileId` — `sources[i]` is the file spans stamp `FileId(i)`.
    pub sources: Vec<(PathBuf, String)>,
    /// Byte-offset <-> LSP position index, parallel to `sources`.
    pub lines: Vec<LineIndex>,
    pub facts: Vec<Fact>,
    /// Diagnostics carrying a span (attachable to a file). Spanless project-level
    /// diagnostics are surfaced separately (as window messages).
    pub diagnostics: Vec<Diagnostic>,
    /// Project-level diagnostics with no span (e.g. a malformed manifest).
    pub project_diagnostics: Vec<Diagnostic>,
}

impl Snapshot {
    /// `FileId` index for a path, matched by canonicalized path.
    pub fn file_id_of(&self, path: &Path) -> Option<usize> {
        let want = canon(path);
        self.sources.iter().position(|(p, _)| canon(p) == want)
    }
}

/// Compile a snapshot rooted at `root`, with `overlays` (canonical path -> unsaved
/// buffer text) taking precedence over on-disk contents.
pub fn compile(root: &Path, overlays: &HashMap<PathBuf, String>) -> Snapshot {
    let mut project_diagnostics = Vec::new();

    // The file set: the project's closed `.bsl` glob when there is a manifest,
    // else just the open buffers (so a lone file still gets facts + diagnostics).
    let paths: Vec<PathBuf> = match based_manifest::discover(root) {
        Ok(project) => project.files.into_iter().map(|f| f.path).collect(),
        Err(diags) => {
            project_diagnostics.extend(diags);
            let mut ps: Vec<PathBuf> = overlays.keys().cloned().collect();
            ps.sort();
            ps
        }
    };

    // Read every file, preferring an open buffer over disk.
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(paths.len());
    for path in paths {
        let text = overlays
            .get(&canon(&path))
            .cloned()
            .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
        sources.push((path, text));
    }

    // Parse each file; collect decls only if every file parsed clean (sema assumes
    // well-formed input, matching the CLI's precondition).
    let mut decls: Vec<Decl> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut parse_ok = true;
    for (i, (_, src)) in sources.iter().enumerate() {
        match based_parser::parse_file(src, FileId(i as u32)) {
            Ok(sf) => decls.extend(sf.decls),
            Err(diags) => {
                diagnostics.extend(diags);
                parse_ok = false;
            }
        }
    }

    let mut facts = Vec::new();
    if parse_ok {
        let (schema, diags) = based_sema::check(&decls);
        diagnostics.extend(diags);
        facts = based_facts::facts(&schema, &decls);
    }

    let lines = sources.iter().map(|(_, s)| LineIndex::new(s)).collect();
    Snapshot {
        sources,
        lines,
        facts,
        diagnostics,
        project_diagnostics,
    }
}

/// Canonicalize for path comparison; fall back to the raw path if the file does
/// not resolve (e.g. an unsaved buffer whose path may not exist on disk yet).
fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Byte-offset <-> LSP `Position` mapping for one file. LSP positions are 0-based
/// `(line, character)` where `character` counts UTF-16 code units (the protocol
/// default); we compute that faithfully so multibyte source lines map correctly.
pub struct LineIndex {
    src: String,
    /// Byte offset of each line's first byte.
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex {
            src: src.to_string(),
            line_starts,
        }
    }

    /// Byte offset -> `(line, utf16-character)`.
    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.src.len());
        // Last line whose start is <= offset.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let start = self.line_starts[line];
        let character = self.src[start..offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Position::new(line as u32, character as u32)
    }

    /// `(line, utf16-character)` -> byte offset.
    pub fn offset(&self, pos: Position) -> usize {
        let line = pos.line as usize;
        let Some(&start) = self.line_starts.get(line) else {
            return self.src.len();
        };
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.src.len());
        let mut units = 0usize;
        for (i, c) in self.src[start..end].char_indices() {
            if units >= pos.character as usize {
                return start + i;
            }
            units += char::len_utf16(c);
        }
        end
    }

    /// Position at the end of the line containing `offset` (trailing newline
    /// excluded) — where a per-declaration inlay hint reads best.
    pub fn end_of_line(&self, offset: usize) -> Position {
        let offset = offset.min(self.src.len());
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let start = self.line_starts[line];
        let end = self.src[start..]
            .find('\n')
            .map(|n| start + n)
            .unwrap_or(self.src.len());
        self.position(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn position_offset_round_trip_ascii() {
        let src = "Order {\n  name: text\n}\n";
        let idx = LineIndex::new(src);
        // "name" starts at line 1 (0-based), char 2.
        let at = src.find("name").unwrap();
        assert_eq!(idx.position(at), Position::new(1, 2));
        assert_eq!(idx.offset(Position::new(1, 2)), at);
    }

    #[test]
    fn position_counts_utf16_code_units() {
        // "é" is one UTF-16 unit but two UTF-8 bytes; "𐐷" is two UTF-16 units.
        let src = "// é𐐷 x\n";
        let idx = LineIndex::new(src);
        let x = src.find('x').unwrap();
        // chars before x on the line: '/', '/', ' ', 'é'(1), '𐐷'(2), ' ' = 7 units.
        assert_eq!(idx.position(x), Position::new(0, 7));
        assert_eq!(idx.offset(Position::new(0, 7)), x);
    }

    #[test]
    fn end_of_line_skips_the_newline() {
        let src = "Order {\n  x: int\n}\n";
        let idx = LineIndex::new(src);
        let brace = src.find('{').unwrap();
        // End of line 0 = after "Order {" (7 chars), before the '\n'.
        assert_eq!(idx.end_of_line(brace), Position::new(0, 7));
    }

    #[test]
    fn compile_commerce_has_facts_and_no_diagnostics() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile(&root, &HashMap::new());
        assert!(snap.diagnostics.is_empty(), "{:?}", snap.diagnostics);
        assert!(snap.project_diagnostics.is_empty());
        assert!(!snap.sources.is_empty());
        // The inferred inverse on `Order.items` is surfaced.
        assert!(
            snap.facts.iter().any(|f| f.label.contains("via order")),
            "{:?}",
            snap.facts
        );
    }
}
