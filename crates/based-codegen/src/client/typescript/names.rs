/// Words a TypeScript identifier may not be.
const RESERVED: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// A name usable as a TypeScript binding: reserved words gain a trailing `_`.
pub(super) fn binding(name: &str) -> String {
    if RESERVED.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// A builder method name; the builder's own methods keep theirs.
pub(super) fn setter_name(field: &str) -> String {
    if ["insert", "on_shard", "shard", "row"].contains(&field) {
        format!("set_{field}")
    } else {
        field.to_string()
    }
}
