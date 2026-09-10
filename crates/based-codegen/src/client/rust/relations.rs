use super::*;

/// The far-side model of a flatten path (`edge.far`) — the last segment's relation
/// target, walking each hop through the schema. `None` on a malformed path (sema
/// reports it).
pub(super) fn flatten_far_model<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    path: &Path,
) -> Option<&'a RModel> {
    let mut cur = model?.name.clone();
    let mut out = None;
    for seg in &path.segments {
        let target = match schema.model(&cur)?.member(&seg.node).map(|m| &m.kind)? {
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => target,
            MemberKind::Scalar { .. } => return None,
        };
        out = schema.model(target);
        cur = target.clone();
    }
    out
}

/// The target model + `optional` of a **to-one** relation field, or `None` for a
/// scalar, an unknown field, or a to-**many** edge (a Forward is always to-one; an
/// Inverse is to-one only when its paired forward FK is unique — a one-to-one back
/// edge, which may be absent, hence optional). Mirrors the SQL side's `enter_to_one`.
pub(super) fn to_one_relation<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    field: &str,
) -> Option<(&'a RModel, bool)> {
    match model?.member(field).map(|m| &m.kind)? {
        MemberKind::Forward {
            target, optional, ..
        } => schema.model(target).map(|t| (t, *optional)),
        MemberKind::Inverse { target, via } => {
            let t = schema.model(target)?;
            t.is_unique(via).then_some((t, true))
        }
        MemberKind::Scalar { .. } => None,
    }
}

/// The target model of a to-**many** relation field (an Inverse collection — its
/// paired forward FK is *not* unique), or `None` for a scalar / to-one edge. Mirrors
/// the SQL side's `to_many_edge`; the client renders it `Vec<Sub>`.
pub(super) fn to_many_relation<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    field: &str,
) -> Option<&'a RModel> {
    match model?.member(field).map(|m| &m.kind)? {
        MemberKind::Inverse { target, via } => {
            let t = schema.model(target)?;
            (!t.is_unique(via)).then_some(t)
        }
        _ => None,
    }
}
