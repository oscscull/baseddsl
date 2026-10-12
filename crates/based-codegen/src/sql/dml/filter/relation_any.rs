//! Lower `rel.any(pred)` to a correlated `EXISTS` subquery over the related rows.

use super::super::*;

impl<'a> Select<'a> {
    /// `EXISTS (SELECT 1 FROM <child> WHERE <correlation> [AND <tombstone/scope>] [AND
    /// <pred>])`. Leading to-one hops join in the current scope; the last, to-many hop is
    /// the subquery, whose own reaches join inside it.
    pub(super) fn relation_any(
        &mut self,
        path: &Path,
        pred: Option<&Predicate>,
        model: &RModel,
    ) -> String {
        let Some((last, hops)) = path.segments.split_last() else {
            return "1 = 0".to_string();
        };
        let Some(mut cur) = self.schema.model(&model.name) else {
            return "1 = 0".to_string();
        };
        let mut alias = self.root_alias.clone();
        let mut prefix = String::new();
        for seg in hops {
            // Sema guarantees every leading hop is a to-one relation.
            let Some((a, p, m)) = self.enter_to_one(&seg.node, &alias, &prefix, cur) else {
                return "1 = 0".to_string();
            };
            (alias, prefix, cur) = (a, p, m);
        }
        let Some((child, via, _)) = self.to_many_edge(&last.node, cur) else {
            return "1 = 0".to_string();
        };

        self.sub_counter += 1;
        let child_alias = format!("s{}_{}", self.sub_counter, child.table);
        let mut sub = self.spawn_child(child_alias.clone());
        let mut wheres = vec![self.to_many_correlation(child, &via, cur, &child_alias, &alias)];
        if let Some(sd) = &child.soft_delete {
            wheres.push(soft_pred(self.dialect, &child_alias, child, sd));
        }
        if let Some(scope) = sub.scope_join_pred(&child_alias, child) {
            wheres.push(scope);
        }
        if let Some(p) = pred {
            wheres.push(sub.predicate(p, child));
        }
        self.sub_counter = sub.sub_counter;

        let mut sql = format!(
            "EXISTS (SELECT 1 FROM {} AS {}",
            self.qt(child),
            self.q(&child_alias)
        );
        push_joins(&mut sql, self.dialect, &sub.joins);
        sql.push_str(&format!(" WHERE {})", wheres.join(" AND ")));
        sql
    }
}
