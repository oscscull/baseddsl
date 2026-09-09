//! The `with count` companion query: the live-row total.

use super::*;

/// The `with count` companion query: the live-row total (soft-delete/scope applied, no
/// LIMIT). Under `distinct` the total must count the **deduped** projected rows, not the
/// raw joined rows — so it wraps the `SELECT DISTINCT <projection>` in a derived table
/// (a bare `COUNT(*)` would overcount and break the client's page-count math). The plain
/// path counts rows directly.
pub(crate) fn count_query(
    sel: &Select,
    root: &RModel,
    projection: &str,
    wheres: &[String],
    distinct: bool,
) -> String {
    let mut cnt = if distinct {
        let mut inner = format!("SELECT DISTINCT\n{projection}\nFROM {}", sel.qt(root));
        push_joins(&mut inner, sel.dialect, &sel.joins);
        if !wheres.is_empty() {
            inner.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
        }
        format!(
            "SELECT COUNT(*) AS {}\nFROM ({inner}) AS {}",
            sel.q("count"),
            sel.q("distinct_rows")
        )
    } else {
        let mut cnt = format!(
            "SELECT COUNT(*) AS {}\nFROM {}",
            sel.q("count"),
            sel.qt(root)
        );
        push_joins(&mut cnt, sel.dialect, &sel.joins);
        if !wheres.is_empty() {
            cnt.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
        }
        cnt
    };
    cnt.push_str(";\n");
    cnt
}
