//! Schema-path type/nullability inference: the primitive, scalar primitive, or nullability a
//! dotted path terminates in, and whether a stored column is a plain `json` scalar.

use super::*;

/// The primitive a dotted sort path terminates in, walked against the schema: a scalar
/// is its own primitive, a relation terminal is the FK it sorts by (a uuid). An
/// unresolved path (sema already flagged) falls back to `Text` so lowering terminates.
pub(crate) fn path_primitive(schema: &CheckedSchema, root: &RModel, path: &Path) -> Primitive {
    let mut cur = root;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind) {
            Some(MemberKind::Scalar { ty, .. }) => return *ty,
            Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
                if last {
                    // The FK's primitive mirrors the target model's primary-key type
                    // (a serial target → an `int` FK, a uuid target → uuid).
                    return schema
                        .model(target)
                        .and_then(RModel::pk_member)
                        .and_then(|m| match &m.kind {
                            MemberKind::Scalar { ty, .. } => Some(*ty),
                            _ => None,
                        })
                        .unwrap_or(Primitive::Uuid);
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return Primitive::Text,
                }
            }
            None => return Primitive::Text,
        }
    }
    Primitive::Text
}

/// The primitive a dotted path lands on, walking relations, or `None` when it doesn't
/// terminate on a scalar. Shared by the computed-field type inference.
pub(crate) fn path_scalar_primitive(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    path: &Path,
) -> Option<Primitive> {
    let mut cur = model?;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind)? {
            MemberKind::Scalar { ty, .. } if last => return Some(*ty),
            MemberKind::Scalar { .. } => return None,
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                if last {
                    return None;
                }
                cur = schema.model(target)?;
            }
        }
    }
    None
}

/// Whether a dotted sort path terminates in a nullable column: a scalar's `optional`, a
/// forward relation's nullable FK. A to-many/inverse terminal or an unresolved path is
/// treated as non-nullable (the keyset comparison stays in its plain, non-NULL form).
pub(crate) fn path_nullable(schema: &CheckedSchema, root: &RModel, path: &Path) -> bool {
    let mut cur = root;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind) {
            Some(MemberKind::Scalar { optional, .. }) => return *optional,
            Some(MemberKind::Forward {
                target, optional, ..
            }) => {
                if last {
                    return *optional;
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return false,
                }
            }
            Some(MemberKind::Inverse { target, .. }) => match schema.model(target) {
                Some(m) if !last => cur = m,
                _ => return false,
            },
            None => return false,
        }
    }
    false
}

/// Whether `field` on `model` is a plain `json`-typed stored column (not an enum-, raw-, or
/// generated-typed column that merely rides the text path).
pub(crate) fn is_json_scalar(model: &RModel, field: &str) -> bool {
    matches!(
        model.member(field).map(|m| &m.kind),
        Some(MemberKind::Scalar {
            ty: Primitive::Json,
            raw_type: None,
            ..
        })
    )
}
