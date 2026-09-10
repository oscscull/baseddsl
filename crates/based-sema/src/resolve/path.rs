use super::*;

/// Resolve `path` from model `start`. Reports the first failure and returns `None`.
pub fn resolve_path(path: &Path, start: usize, cx: &Cx, sink: &mut Sink) -> Option<Terminal> {
    let mut cur = start;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let Some(mem) = cx.model(cur).member(&seg.node) else {
            sink.error(
                code::UNKNOWN_FIELD,
                seg.span,
                format!("`{}` has no field `{}`", cx.model(cur).name, seg.node),
            );
            return None;
        };
        let last = i + 1 == n;
        match &mem.kind {
            MemberKind::Scalar { ty, raw_type, .. } => {
                if last {
                    return Some(match raw_type {
                        Some(spec) => Terminal::Opaque(spec.render()),
                        None => Terminal::Scalar(*ty),
                    });
                }
                sink.error(
                    code::TRAVERSE_SCALAR,
                    seg.span,
                    format!("cannot traverse into scalar column `{}`", seg.node),
                );
                return None;
            }
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                if last {
                    return Some(Terminal::Relation(target.clone()));
                }
                // Target existence is validated during model checking; if it is
                // missing, that error already fired — just stop here.
                cur = cx.find(target)?;
            }
        }
    }
    None
}
