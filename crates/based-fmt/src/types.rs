//! Reprint a type expression (primitive, model, or raw spec, with optional/array
//! markers) and the primitive-type keywords.

use based_ast::*;

pub(crate) fn type_expr(t: &TypeExpr) -> String {
    let mut s = match &t.base {
        BaseType::Primitive(p) => primitive(*p),
        BaseType::Model(m) => m.node.clone(),
        BaseType::Raw(spec) => spec.render(),
    };
    if t.optional {
        s.push('?');
    }
    if t.many {
        s.push_str("[]");
    }
    s
}

fn primitive(p: Primitive) -> String {
    match p {
        Primitive::Text => "text".into(),
        Primitive::Int => "int".into(),
        Primitive::Bool => "bool".into(),
        Primitive::Timestamp => "timestamp".into(),
        Primitive::Date => "date".into(),
        Primitive::Time => "time".into(),
        Primitive::Bytes => "bytes".into(),
        Primitive::Json => "json".into(),
        Primitive::Uuid => "uuid".into(),
        Primitive::Id => "Id".into(),
        Primitive::Ulid => "ulid".into(),
        Primitive::Serial => "serial".into(),
        Primitive::Float => "float".into(),
        Primitive::Decimal { precision, scale } => format!("decimal({precision}, {scale})"),
    }
}
