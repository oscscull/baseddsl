//! Classify a to-many relation edge and build its aggregating-subquery correlation predicate.

use super::*;

impl<'a> Select<'a> {
    /// A to-**many** relation edge `field` on `model` (an Inverse whose paired forward FK
    /// is *not* unique — a genuine collection), as `(child_model, back_fk_column,
    /// edge_sort)`. `None` for a scalar, a forward relation, or a to-one inverse (those
    /// are `enter_to_one`'s). The back FK is the column on the child carrying the
    /// relation back to `model` (`OrderItem.order` → `order_id`), used to correlate the
    /// aggregating subquery. `edge_sort` is the edge's own relation `@sort` (empty when
    /// undeclared) — the traversal tier of the sort cascade ordering the nested array.
    pub(crate) fn to_many_edge(
        &self,
        field: &str,
        model: &'a RModel,
    ) -> Option<(&'a RModel, String, &'a [SortTerm])> {
        let member = model.member(field)?;
        match &member.kind {
            MemberKind::Inverse { target, via } => {
                let tmodel = self.schema.model(target)?;
                if tmodel.is_unique(via) {
                    return None; // to-one back edge — handled by `enter_to_one`.
                }
                Some((tmodel, via.clone(), &member.sort))
            }
            _ => None,
        }
    }

    /// The `(child_fk_col, parent_pk_col)` correlation pairs of a to-many edge — the child's
    /// back-FK column(s) referencing the parent's primary key. One pair for a single-column
    /// key, several for a composite key.
    fn to_many_pairs(
        &self,
        child: &RModel,
        via_field: &str,
        parent: &RModel,
    ) -> Vec<(String, String)> {
        match child.member(via_field) {
            Some(m) if matches!(m.kind, MemberKind::Forward { .. }) => self.fk_join_pairs(m),
            _ => vec![(format!("{via_field}_id"), pk_col(parent))],
        }
    }

    /// The correlation predicate tying a to-many child row (at `child_alias`) to its
    /// parent (at `outer_alias`): the child's back-FK column(s) equal the parent's
    /// primary key, or — when the `via` forward carries a custom `on:` — that declared
    /// condition (near = the FK-holding child, far = the parent).
    pub(crate) fn to_many_correlation(
        &self,
        child: &'a RModel,
        via_field: &str,
        parent: &RModel,
        child_alias: &str,
        outer_alias: &str,
    ) -> String {
        if let Some(MemberKind::Forward {
            custom_on: Some(pred),
            ..
        }) = child.member(via_field).map(|m| &m.kind)
        {
            return render_join_on(self.dialect, pred, &child.table, child_alias, outer_alias);
        }
        self.to_many_pairs(child, via_field, parent)
            .iter()
            .map(|(child_fk, parent_pk)| {
                format!(
                    "{} = {}",
                    self.qcol(child_alias, child_fk),
                    self.qcol(outer_alias, parent_pk)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}
