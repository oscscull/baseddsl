//! `%XX` decoding of a `sqlite:` URL path, matching sqlx's reading of the same URL.

use super::SqliteLocationError;

pub(super) fn decode(path: &str) -> Result<String, SqliteLocationError> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let byte = bytes
                .get(i + 1..i + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or(SqliteLocationError::InvalidEscape)?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| SqliteLocationError::InvalidEscape)
}
