//! Resolve a relation field/path to its target model and arity (schema-graph walk, no SQL).

use super::*;

/// A relation field's target model and whether it is to-**many** (an inverse edge → a JSON
/// array in the result) rather than to-one. `None` for a non-relation field.
pub(crate) fn relation_target<'a>(model: &'a RModel, field: &str) -> Option<(&'a str, bool)> {
    match &model.member(field)?.kind {
        MemberKind::Forward { target, .. } => Some((target.as_str(), false)),
        MemberKind::Inverse { target, .. } => Some((target.as_str(), true)),
        MemberKind::Scalar { .. } => None,
    }
}

/// The model a relation `path` terminates on (walking each edge to its target).
pub(crate) fn relation_terminal_model<'a>(
    schema: &'a CheckedSchema,
    model: &'a RModel,
    path: &Path,
) -> Option<&'a RModel> {
    let mut cur = model;
    for seg in &path.segments {
        let (target, _) = relation_target(cur, &seg.node)?;
        cur = schema.model(target)?;
    }
    Some(cur)
}
