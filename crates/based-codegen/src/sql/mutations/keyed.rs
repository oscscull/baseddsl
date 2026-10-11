//! Whether a write's `where` pins at most one row.

use super::*;

/// The predicate binds a unique column, or every primary-key member, by a conjunctive
/// equality — a keyed single-row write. Under `-> ok`, only a keyed write reports an
/// absent row as `not_found`; a filtered bulk write matching nothing is a no-op success.
pub(crate) fn where_is_keyed(pred: &Predicate, model: &RModel) -> bool {
    let pk = model.pk_members();
    model
        .unique_cols
        .iter()
        .any(|col| bound_key(pred, col).is_some())
        || (!pk.is_empty() && pk.iter().all(|key| bound_key(pred, &key.name).is_some()))
}

/// Only equalities required by conjunction can uniquely identify the written row.
/// A key inside OR/NOT, or a value read from a changing column, is not a stable key.
pub(crate) fn bound_key<'a>(pred: &'a Predicate, field: &str) -> Option<&'a Predicate> {
    match pred {
        Predicate::And(a, b) => bound_key(a, field).or_else(|| bound_key(b, field)),
        Predicate::Cmp {
            path,
            op: Op::Eq,
            value: Value::Param(_) | Value::Lit(_),
        } if local_field(path) == Some(field) => Some(pred),
        _ => None,
    }
}

pub(crate) fn local_field(path: &Path) -> Option<&str> {
    match path.segments.as_slice() {
        [field] => Some(field.node.as_str()),
        _ => None,
    }
}
