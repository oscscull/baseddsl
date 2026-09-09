use super::*;

pub(crate) fn join_path(p: &Path) -> String {
    p.segments
        .iter()
        .map(|s| s.node.as_str())
        .collect::<Vec<_>>()
        .join(".")
}
