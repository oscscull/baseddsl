use super::*;

pub(super) fn check_param(p: &Param, ti: usize, infer: bool, cx: &Cx, sink: &mut Sink) {
    let m = cx.model(ti);
    // The column/edge this param maps onto — its type is what an explicit annotation must
    // agree with. `None` when the mapping is unresolved (error already reported) or the
    // param has no column mapping.
    let mapped: Option<resolve::Mapped> = match &p.binding {
        Some(ParamBinding::Edge(edge)) => match m.member(&edge.node).map(|mm| &mm.kind) {
            Some(k) if k.is_relation() => Some(resolve::Mapped::Relation(k.target().unwrap())),
            Some(_) => {
                sink.error(
                    code::BINDING_EDGE,
                    edge.span,
                    format!(
                        "`{}` is a column, not a relation, so a param can't bind via it",
                        edge.node
                    ),
                );
                None
            }
            None => {
                unknown_field(cx, ti, edge, sink);
                None
            }
        },
        Some(ParamBinding::ColOp { col, .. }) => mapped_member(m, &col.node, cx, ti, col, sink),
        // Bare/inline queries map an unbound param onto a same-named column; block
        // queries reference params via `$`, so a bare param maps to nothing.
        None if infer => match m.member(&p.name.node).map(|mm| &mm.kind) {
            Some(MemberKind::Scalar { ty, .. }) => Some(resolve::Mapped::Scalar(*ty)),
            Some(k @ (MemberKind::Forward { .. } | MemberKind::Inverse { .. })) => {
                Some(resolve::Mapped::Relation(k.target().unwrap()))
            }
            None => {
                sink.error(
                    code::UNKNOWN_FIELD,
                    p.name.span,
                    format!(
                        "param `{}` maps to a same-named column, but `{}` has none",
                        p.name.node, m.name
                    ),
                );
                None
            }
        },
        None => None,
    };

    if let (Some(ann), Some(mapped)) = (&p.ty, mapped) {
        // For a relation binding, resolve the target's single-column key type so an explicit
        // concrete annotation (`uuid`) that contradicts a `serial` key is caught here rather
        // than silently mis-coercing at runtime.
        let target_key = match &mapped {
            resolve::Mapped::Relation(t) => target_key_primitive(cx, t),
            resolve::Mapped::Scalar(_) => None,
        };
        resolve::check_param_type(ann, mapped, target_key, sink);
    }
    if let Some(d) = &p.default {
        resolve::check_default(d, sink);
    }
}

/// Resolve a member by name to its `Mapped` type, reporting an unknown-field error
/// (and returning `None`) when it doesn't exist.
fn mapped_member<'a>(
    m: &'a RModel,
    name: &str,
    cx: &Cx,
    ti: usize,
    at: &Ident,
    sink: &mut Sink,
) -> Option<resolve::Mapped<'a>> {
    match m.member(name).map(|mm| &mm.kind) {
        Some(MemberKind::Scalar { ty, .. }) => Some(resolve::Mapped::Scalar(*ty)),
        Some(k @ (MemberKind::Forward { .. } | MemberKind::Inverse { .. })) => {
            Some(resolve::Mapped::Relation(k.target().unwrap()))
        }
        None => {
            unknown_field(cx, ti, at, sink);
            None
        }
    }
}

/// The single-column primary-key primitive of a relation's target model — what an explicit
/// key annotation on a param binding it must agree with. `None` for a composite-key or
/// keyless target (nothing single to contradict). Runs pre-`resolve_pk_default`, so a bare
/// `id: Id` reads as `Id` (uuid-family); an explicit `serial`/`uuid`/`ulid` reads as itself.
fn target_key_primitive(cx: &Cx, target: &str) -> Option<Primitive> {
    let m = cx.model(cx.find(target)?);
    if m.is_composite_key() {
        return None;
    }
    match m.pk_member().map(|mm| &mm.kind) {
        Some(MemberKind::Scalar { ty, .. }) => Some(*ty),
        _ => None,
    }
}
