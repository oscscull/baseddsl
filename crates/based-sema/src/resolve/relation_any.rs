use super::*;

/// Resolve the relation path of `rel.any(…)` from model `start` to the model whose rows
/// it ranges over. Leading hops must be to-one relations; the last must be to-many.
/// Reports the first failure and returns `None`.
pub fn resolve_any_target(path: &Path, start: usize, cx: &Cx, sink: &mut Sink) -> Option<usize> {
    let mut cur = start;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let model = cx.model(cur);
        let Some(mem) = model.member(&seg.node) else {
            sink.error(
                code::UNKNOWN_FIELD,
                seg.span,
                format!("`{}` has no field `{}`", model.name, seg.node),
            );
            return None;
        };
        let (target, to_many) = match &mem.kind {
            MemberKind::Scalar { .. } => {
                sink.error(
                    code::RELATION_ANY,
                    seg.span,
                    format!(
                        "`.any` needs a to-many relation, but `{}` is a column",
                        seg.node
                    ),
                );
                return None;
            }
            MemberKind::Forward { target, .. } => (target, false),
            MemberKind::Inverse { target, via } => (
                target,
                cx.find(target).is_some_and(|t| !cx.model(t).is_unique(via)),
            ),
        };
        let last = i + 1 == n;
        if last && !to_many {
            sink.error(
                code::RELATION_ANY,
                seg.span,
                format!(
                    "`.any` needs a to-many relation, but `{0}` is to-one — compare its \
                     fields directly (`{0}.<field> = …`)",
                    seg.node
                ),
            );
            return None;
        }
        if !last && to_many {
            sink.error(
                code::RELATION_ANY,
                seg.span,
                format!(
                    "`{0}` is a to-many relation — nest the check (`{0}.any(<next>.any(…))`)",
                    seg.node
                ),
            );
            return None;
        }
        cur = cx.find(target)?;
    }
    Some(cur)
}
