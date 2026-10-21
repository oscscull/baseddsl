use super::*;

/// Every `$p` / `$ctx.field` argument of a filter projection in the shape a query or
/// mutation returns (following `field -> Shape` nests) — the shape-side uses its params bind.
pub(crate) fn returned_shape_param_refs<'a>(decls: &'a [Decl], d: &'a Decl) -> Vec<&'a ParamRef> {
    let ret = match d {
        Decl::Query(q) => &q.ret.ty.node,
        Decl::Mutation(m) => &m.ret.ty.node,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut seen = Vec::new();
    shape_refs_by_name(decls, ret, &mut seen, &mut out);
    out
}

/// Every filter-projection argument ref in shape declarations themselves, unfollowed.
pub(crate) fn shape_decl_param_refs(decls: &[Decl]) -> Vec<&ParamRef> {
    let mut out = Vec::new();
    for d in decls {
        if let Decl::Shape(s) = d {
            body_refs(decls, &s.body, &mut Vec::new(), false, &mut out);
        }
    }
    out
}

fn shape_refs_by_name<'a>(
    decls: &'a [Decl],
    name: &str,
    seen: &mut Vec<&'a str>,
    out: &mut Vec<&'a ParamRef>,
) {
    let Some(s) = decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name => Some(s),
        _ => None,
    }) else {
        return;
    };
    if seen.contains(&s.name.node.as_str()) {
        return;
    }
    seen.push(s.name.node.as_str());
    body_refs(decls, &s.body, seen, true, out);
}

fn body_refs<'a>(
    decls: &'a [Decl],
    body: &'a [ShapeField],
    seen: &mut Vec<&'a str>,
    follow: bool,
    out: &mut Vec<&'a ParamRef>,
) {
    for f in body {
        match f {
            ShapeField::Rename {
                value: ShapeValue::Filter(fp),
                ..
            } => fp.args.iter().for_each(|a| value_param_refs(a, out)),
            ShapeField::Nest { body, .. } | ShapeField::Flatten { body, .. } => {
                body_refs(decls, body, seen, follow, out);
            }
            ShapeField::NestRef { shape, .. } if follow => {
                shape_refs_by_name(decls, &shape.node, seen, out);
            }
            _ => {}
        }
    }
}
