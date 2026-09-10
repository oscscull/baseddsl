use super::*;


/// The DSL keyword vocabulary, derived from the parser's positionally-recognized
/// keywords (there is no `model` keyword — a model is a bare `UpperName { … }`).
pub(super) const KEYWORDS: &[&str] = &[
    "shape",
    "query",
    "mutation",
    "filter",
    "from",
    "guard",
    "unscoped",
    "get",
    "list",
    "distinct",
    "create",
    "update",
    "delete",
    "restore",
    "tx",
    "hard",
    "where",
    "order",
    "group",
    "by",
    "having",
    "page",
    "unindexed",
    "index",
    "unique",
    "using",
    "default",
    "column",
    "asc",
    "desc",
    "offset",
    "with",
    "count",
    "for",
    "nowait",
    "skip",
    "locked",
    "and",
    "or",
    "not",
    "in",
    "has",
    "on",
    "full",
    "stream",
    "raw",
    "read",
    "max_rows",
    "unsafe",
    // computed shape-field conditional (`out = case when … then … else … end`)
    "case",
    "when",
    "then",
    "else",
    "end",
];


/// Primitive type spellings (the `Primitive` variants), offered in type position.
pub(super) const PRIMITIVES: &[&str] = &[
    "text",
    "int",
    "bool",
    "timestamp",
    "date",
    "time",
    "bytes",
    "json",
    "uuid",
    "Id",
];


/// Everything an author writes after `@`: the engine-understood model decorators
/// (`based_sema::KNOWN_DECORATORS`) plus the member-level `@index` and the
/// `@was("old")` rename directive.
pub(super) fn decorator_items() -> Vec<CompletionItem> {
    based_sema::KNOWN_DECORATORS
        .iter()
        .copied()
        .chain(["index", "was"])
        .map(|d| item(d, CompletionItemKind::PROPERTY))
        .collect()
}


/// The dotted identifier chain immediately before a `.` in `head` (which ends with
/// that `.`) — the base path a field completion resolves. `["a", "b"]` for `a.b.`;
/// empty when the char before the `.` is not an identifier (`$ctx` aside).
pub(super) fn trailing_path(head: &str) -> Vec<String> {
    let Some(mut rest) = head.strip_suffix('.') else {
        return Vec::new();
    };
    let mut segs: Vec<String> = Vec::new();
    loop {
        let n: usize = rest
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum();
        if n == 0 {
            break;
        }
        segs.push(rest[rest.len() - n..].to_string());
        rest = &rest[..rest.len() - n];
        match rest.strip_suffix('.') {
            Some(r) => rest = r,
            None => break,
        }
    }
    segs.reverse();
    segs
}


/// One completion item with just a label + kind (no snippet / fuzzy detail).
pub(super) fn item(label: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        ..Default::default()
    }
}
