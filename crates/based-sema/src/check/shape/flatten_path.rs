use super::*;

/// Validate a flatten path (`enrollments.course`): the first segment must be a
/// to-**many** inverse edge (into the junction), and each later segment a forward
/// edge to the next model. Returns the far model (the last segment's target) on a
/// clean path, else reports the offending segment and returns `None`.
pub(super) fn check_flatten_path(
    path: &Path,
    mi: usize,
    cx: &Cx,
    sink: &mut Sink,
) -> Option<usize> {
    let segs = &path.segments;
    let first = &segs[0];
    if segs.len() < 2 {
        sink.error_note(
            code::FLATTEN_SEGMENT,
            first.span,
            format!("`{}` has no forward hop to a far side", first.node),
            "a flattening projection skips a junction: `edge.far { … }` (a to-many edge, then a forward edge)",
        );
        return None;
    }
    let mut cur = match cx.model(mi).member(&first.node).map(|m| &m.kind) {
        Some(MemberKind::Inverse { target, via }) => {
            let ti = cx.find(target)?;
            if cx.model(ti).is_unique(via) {
                sink.error(
                    code::FLATTEN_NOT_TOMANY,
                    first.span,
                    format!(
                        "`{}` is a to-one edge; a flattening projection skips a to-*many* junction",
                        first.node
                    ),
                );
                return None;
            }
            ti
        }
        Some(_) => {
            sink.error(
                code::FLATTEN_NOT_TOMANY,
                first.span,
                format!(
                    "`{}` must be a to-many edge (into the junction) to flatten through it",
                    first.node
                ),
            );
            return None;
        }
        None => {
            unknown_field(cx, mi, first, sink);
            return None;
        }
    };
    for seg in &segs[1..] {
        match cx.model(cur).member(&seg.node).map(|m| &m.kind) {
            Some(MemberKind::Forward { target, .. }) => cur = cx.find(target)?,
            Some(_) => {
                sink.error(
                    code::FLATTEN_SEGMENT,
                    seg.span,
                    format!("`{}` must be a forward relation to the far model", seg.node),
                );
                return None;
            }
            None => {
                unknown_field(cx, cur, seg, sink);
                return None;
            }
        }
    }
    Some(cur)
}
