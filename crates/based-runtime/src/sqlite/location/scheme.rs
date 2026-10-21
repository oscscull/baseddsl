//! The URL scheme a connection value starts with, if any.

/// The `scheme` of a `scheme:` prefix (RFC 3986 characters). Single letters are excluded
/// so a Windows drive path (`C:\db`) stays a path.
pub(super) fn of(value: &str) -> Option<&str> {
    let (candidate, _) = value.split_once(':')?;
    let mut chars = candidate.chars();
    let starts_alpha = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest_valid = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (candidate.len() > 1 && starts_alpha && rest_valid).then_some(candidate)
}
