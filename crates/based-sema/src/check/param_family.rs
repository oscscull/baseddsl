use super::*;

/// The family a callable param carries: its annotation, else the column it binds (`-> edge`,
/// `op col`, or — on a bare/inline query — its same-named column). `None` when untyped
/// and unbound.
pub(super) fn param_family(p: &Param, ti: usize, infer: bool, cx: &Cx) -> Option<resolve::Family> {
    if let Some(t) = &p.ty {
        return match &t.base {
            BaseType::Primitive(prim) => Some(resolve::prim_family(*prim)),
            BaseType::Model(name) => match cx.enum_(&name.node) {
                Some(e) if e.is_int() => Some(resolve::Family::Numeric),
                Some(_) => Some(resolve::Family::Textual),
                None => Some(resolve::Family::Key),
            },
            BaseType::Raw(_) => None,
        };
    }
    let field = match &p.binding {
        Some(ParamBinding::Edge(e)) => &e.node,
        Some(ParamBinding::ColOp { col, .. }) => &col.node,
        None if infer => &p.name.node,
        None => return None,
    };
    resolve::member_family(&cx.model(ti).member(field)?.kind)
}
