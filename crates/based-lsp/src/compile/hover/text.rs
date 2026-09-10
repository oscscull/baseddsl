use super::*;


/// A `TypeExpr` as source writes it: base spelling + `?` (optional) + `[]` (many).
pub(super) fn type_str(ty: &TypeExpr) -> String {
    let mut s = match &ty.base {
        BaseType::Primitive(p) => primitive_str(*p),
        BaseType::Model(id) => id.node.clone(),
        BaseType::Raw(spec) => spec.render(),
    };
    if ty.optional {
        s.push('?');
    }
    if ty.many {
        s.push_str("[]");
    }
    s
}


/// Primitive → its DSL spelling (`Id` keeps its casing, the rest lowercase).
pub(super) fn primitive_str(p: Primitive) -> String {
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


/// `$ctx.org` and the like, from a `ParamRef` (`$` + name + dotted path).
pub(super) fn paramref_str(pr: &ParamRef) -> String {
    let mut s = format!("${}", pr.name.node);
    for seg in &pr.path {
        s.push('.');
        s.push_str(&seg.node);
    }
    s
}


/// A binding operator's DSL spelling.
pub(super) fn op_str(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "~",
        Op::In => "in",
        Op::Has => "has",
    }
}


/// What a binding operator means, for the binding hover.
pub(super) fn op_gloss(op: Op) -> &'static str {
    match op {
        Op::Has => "containment (array/json); the column is the left operand",
        Op::In => "membership; the column is the left operand",
        Op::Like => "SQL `LIKE`, pattern passed verbatim; the column is the left operand",
        _ => "the column is the left operand",
    }
}


/// A field's hover: its `name: Type` signature, plus a cardinality note for relations.
pub(super) fn field_hover(f: &Field) -> String {
    let sig = format!("{}: {}", f.name.node, type_str(&f.ty));
    match &f.ty.base {
        BaseType::Model(m) => {
            let card = if f.ty.many { "to-many" } else { "to-one" };
            format!("```based\n{sig}\n```\n{card} relation to `{}`", m.node)
        }
        BaseType::Raw(_) => format!(
            "```based\n{sig}\n```\nopaque column — the engine stores this DB type verbatim \
             and never reads into it"
        ),
        BaseType::Primitive(_) => format!("```based\n{sig}\n```"),
    }
}


/// A model's hover: `model Name` and its declared-field count.
pub(super) fn model_hover(m: &Model) -> String {
    let n = m
        .members
        .iter()
        .filter(|mem| matches!(mem, Member::Field(_)))
        .count();
    let plural = if n == 1 { "" } else { "s" };
    format!("```based\nmodel {}\n```\n{n} field{plural}", m.name.node)
}


/// A shape's hover: `shape Name from Model`.
pub(super) fn shape_hover(s: &Shape) -> String {
    format!("```based\nshape {} from {}\n```", s.name.node, s.from.node)
}


/// An enum's hover: `enum Name { a, b, c }` (variant names, a compact closed set).
pub(super) fn enum_hover(e: &EnumDecl) -> String {
    let names = e
        .variants
        .iter()
        .map(|v| v.name.node.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("```based\nenum {} {{ {names} }}\n```", e.name.node)
}


/// A scope's hover: `scope Name (col: Type = $ctx.field, …)`.
pub(super) fn scope_hover(s: &ScopeDecl) -> String {
    let terms = s
        .terms
        .iter()
        .map(|t| {
            format!(
                "{}: {} = {}",
                t.col.node,
                type_str(&t.ty),
                paramref_str(&t.ctx)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("```based\nscope {} ({terms})\n```", s.name.node)
}


/// A query's hover: `query name(params) -> Ret[]` (or `-> stream Ret`).
pub(super) fn query_hover(q: &Query) -> String {
    let stream = if q.ret.stream { "stream " } else { "" };
    let card = if q.ret.many { "[]" } else { "" };
    format!(
        "```based\nquery {}({}) -> {stream}{}{card}\n```",
        q.name.node,
        params_str(&q.params),
        q.ret.ty.node,
    )
}


/// A mutation's hover: `mutation name(params) -> Ret[]`.
pub(super) fn mutation_hover(m: &Mutation) -> String {
    let card = if m.ret.many { "[]" } else { "" };
    format!(
        "```based\nmutation {}({}) -> {}{card}\n```",
        m.name.node,
        params_str(&m.params),
        m.ret.ty.node,
    )
}


/// A filter's hover: `filter name(params)`.
pub(super) fn filter_hover(f: &NamedFilter) -> String {
    format!(
        "```based\nfilter {}({})\n```",
        f.name.node,
        params_str(&f.params)
    )
}


/// A parameter list rendered `name: Type` (type dropped when the param is untyped).
pub(super) fn params_str(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| match &p.ty {
            Some(t) => format!("{}: {}", p.name.node, type_str(t)),
            None => p.name.node.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}
