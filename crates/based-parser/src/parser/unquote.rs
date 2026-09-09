/// Strip surrounding quotes and unescape `\"` / `\\` from a `STRING` slice.
pub(crate) fn unquote(s: &str) -> String {
    let inner = &s[1..s.len().saturating_sub(1)];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(esc) => out.push(esc),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
