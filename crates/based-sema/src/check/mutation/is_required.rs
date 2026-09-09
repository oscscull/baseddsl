use super::*;

/// A column the caller must supply on `create`: a non-optional scalar with no
/// default, or a non-optional forward FK (a custom-join edge has no FK column to
/// set, so it is excluded).
pub(super) fn is_required(kind: &MemberKind) -> bool {
    match kind {
        // A generated column is derived by the DB — never supplied on create, even though it
        // is NOT NULL and carries no default.
        MemberKind::Scalar {
            generated: Some(_), ..
        } => false,
        MemberKind::Scalar {
            optional, default, ..
        } => !*optional && default.is_none(),
        MemberKind::Forward {
            optional,
            custom_on,
            ..
        } => !*optional && custom_on.is_none(),
        MemberKind::Inverse { .. } => false,
    }
}
