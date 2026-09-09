use super::*;

/// True when a shape body carries a top-level aggregate field (`= count()` / `= sum(…)`),
/// making it an aggregate shape — a projection over groups, paired with a
/// query's `group by` / `having`.
pub(crate) fn is_agg_shape(body: &[ShapeField]) -> bool {
    body.iter().any(|f| {
        matches!(
            f,
            ShapeField::Rename {
                value: ShapeValue::Agg(_),
                ..
            }
        )
    })
}
