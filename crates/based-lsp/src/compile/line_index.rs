use super::*;

/// The identifier extent (byte range) the cursor at `offset` sits in, or `None` off
/// any word. Identifiers are ASCII, so a non-ASCII byte (high bit set) stops the walk
/// — a byte scan over identifier characters is safe. Shared by prepareRename (the
/// range it offers) and selection ranges (the token level).
pub(crate) fn word_extent(src: &str, offset: u32) -> Option<(usize, usize)> {
    let bytes = src.as_bytes();
    let off = (offset as usize).min(src.len());
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = off;
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = off;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    (start != end).then_some((start, end))
}

/// A source `Span`'s byte range as an LSP `Range` via the owning file's index.
pub(crate) fn span_range(span: Span, idx: &LineIndex) -> Range {
    Range::new(
        idx.position(span.start as usize),
        idx.position(span.end as usize),
    )
}

/// The byte offset of the start of the line containing `off` (the char after the
/// preceding newline, or 0). Where a leading `@was` decorator line is inserted.
pub(crate) fn line_start(src: &str, off: u32) -> usize {
    let o = (off as usize).min(src.len());
    src[..o].rfind('\n').map_or(0, |i| i + 1)
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
        Self {
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
            .map_or(self.src.len(), |n| start + n);
        self.position(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
