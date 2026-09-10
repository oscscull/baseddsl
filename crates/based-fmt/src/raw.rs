//! Reprint a raw-SQL fragment byte-for-byte: verbatim text, `${param}` interpolations,
//! and `{engine}` placeholders, wrapped in backticks.

use based_ast::*;

pub(crate) fn raw_sql(r: &RawSql) -> String {
    let mut s = String::from("raw`");
    for part in &r.parts {
        match part {
            RawPart::Text(t) => s.push_str(t),
            RawPart::Param(pr) => {
                s.push_str("${");
                s.push_str(&pr.name.node);
                for seg in &pr.path {
                    s.push('.');
                    s.push_str(&seg.node);
                }
                s.push('}');
            }
            RawPart::Engine(id) => {
                s.push('{');
                s.push_str(&id.node);
                s.push('}');
            }
        }
    }
    s.push('`');
    s
}
