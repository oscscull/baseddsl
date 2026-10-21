use super::ExplainedStmt;
use crate::scan::{to_positional, Bound};
use based_codegen::Dialect;

/// Rewrite `:name` placeholders exactly as execution does, labelling each bind.
pub(super) fn positional(
    label: &str,
    sql: &str,
    dialect: Dialect,
    param_label: impl Fn(&str) -> String,
) -> ExplainedStmt {
    let (sql, params) = to_positional(sql, dialect, |name| Some(Bound::One(param_label(name))))
        .expect("every placeholder resolves to a label");
    ExplainedStmt {
        label: label.to_string(),
        sql: sql.trim_end().to_string(),
        params,
    }
}
