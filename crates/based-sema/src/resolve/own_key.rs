use super::*;

/// The model whose whole primary key `path` lands on (`id`, `author.id`), so a value
/// compared there is that model's key. `None` for any other column, a relation edge,
/// a composite key's part, or a keyless model. Read-only; reports nothing.
pub(crate) fn own_key_model(path: &Path, start: usize, cx: &Cx) -> Option<String> {
    let (last, hops) = path.segments.split_last()?;
    let mut cur = start;
    for seg in hops {
        match &cx.model(cur).member(&seg.node)?.kind {
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                cur = cx.find(target)?;
            }
            MemberKind::Scalar { .. } => return None,
        }
    }
    member_key_model(cx.model(cur), &last.node)
}

/// `model`'s name when `field` is its whole single-column primary key.
pub(crate) fn member_key_model(model: &RModel, field: &str) -> Option<String> {
    let is_key = !model.is_composite_key() && model.pk_field() == Some(field);
    let scalar = matches!(
        model.member(field).map(|m| &m.kind),
        Some(MemberKind::Scalar { raw_type: None, .. })
    );
    (is_key && scalar).then(|| model.name.clone())
}
