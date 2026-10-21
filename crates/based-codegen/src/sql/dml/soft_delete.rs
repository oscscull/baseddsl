//! The soft-delete tombstone predicate injected into WHERE and joined ON.

use super::*;

/// Soft-delete predicate for a table alias: the model's `read:` override when declared.
pub(crate) fn soft_pred(dialect: Dialect, alias: &str, model: &RModel, sd: &SoftDelete) -> String {
    if let Some(raw) = &sd.overrides.read {
        return format!("({})", render_raw(dialect, raw, alias, Some(model)));
    }
    let col = column_of(model, &sd.field);
    match sd.mode {
        SoftMode::Timestamp => format!("{} IS NULL", dialect.qcol(alias, &col)),
        SoftMode::Bool => format!(
            "{} = {}",
            dialect.qcol(alias, &col),
            dialect.bool_lit(false)
        ),
    }
}
