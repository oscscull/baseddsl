//! Construct a synthetic single-segment path for the call sites that resolve one field name.

use super::*;

/// A one-segment path, for the many call sites that resolve a single field name.
pub(crate) fn single(name: &str) -> Path {
    Path {
        segments: vec![Spanned {
            node: name.to_string(),
            span: NO_SPAN,
        }],
    }
}

const NO_SPAN: Span = Span {
    file: FileId(0),
    start: 0,
    end: 0,
};
