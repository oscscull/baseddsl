//! Detect declaration keywords conservatively; do not pretend to parse SQL expressions.
use std::collections::BTreeSet;

pub(crate) struct Keywords(BTreeSet<String>);

impl Keywords {
    pub(crate) fn read(sql: &str) -> Self {
        let bytes = sql.as_bytes();
        let mut cursor = 0;
        let mut words = BTreeSet::new();
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'\'' | b'"' | b'`' | b'[' => cursor = quoted_end(bytes, cursor),
                b'-' if bytes.get(cursor + 1) == Some(&b'-') => {
                    cursor = bytes[cursor..]
                        .iter()
                        .position(|byte| *byte == b'\n')
                        .map_or(bytes.len(), |length| cursor + length + 1);
                }
                b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                    cursor = bytes[cursor + 2..]
                        .windows(2)
                        .position(|pair| pair == b"*/")
                        .map_or(bytes.len(), |length| cursor + length + 4);
                }
                byte if word(byte) => {
                    let start = cursor;
                    while cursor < bytes.len() && word(bytes[cursor]) {
                        cursor += 1;
                    }
                    words.insert(sql[start..cursor].to_ascii_uppercase());
                }
                _ => cursor += 1,
            }
        }
        Self(words)
    }

    pub(crate) fn has(&self, keyword: &str) -> bool {
        self.0.contains(keyword)
    }
}

fn word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 128
}

fn quoted_end(bytes: &[u8], start: usize) -> usize {
    let end = match bytes[start] {
        b'[' => b']',
        quote => quote,
    };
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        if bytes[cursor] != end {
            cursor += 1;
            continue;
        }
        if end != b']' && bytes.get(cursor + 1) == Some(&end) {
            cursor += 2;
            continue;
        }
        return cursor + 1;
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::Keywords;
    #[test]
    fn quoted_names_literals_comments_and_unicode_are_not_keywords() {
        let words = Keywords::read("CREATE TABLE t(\"CHECK\" TEXT DEFAULT 'COLLATE', `AUTOINCREMENT` INT, [DEFERRABLE] INT, åCHECK INT) /* CHECK */ -- COLLATE\n STRICT");
        assert!(words.has("STRICT"));
        for keyword in ["CHECK", "COLLATE", "DEFERRABLE", "AUTOINCREMENT"] {
            assert!(!words.has(keyword));
        }
    }
}
