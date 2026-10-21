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
        self.relation_exists(path, model, |sub, child| {
            pred.map(|p| sub.predicate(p, child))
        })
    }

    /// The `EXISTS` over the to-many edge `path` ends on, its extra condition lowered by
    /// `inner` in the child's own scope (`None` = existence only).
    pub(super) fn relation_exists(
        &mut self,
        path: &Path,
        model: &RModel,
        inner: impl FnOnce(&mut Self, &'a RModel) -> Option<String>,
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
        let mut filters = Vec::new();
        if let Some(sd) = &child.soft_delete {
            filters.push(soft_pred(self.dialect, &child_alias, child, sd));
        }
        if let Some(scope) = sub.scope_join_pred(&child_alias, child) {
            filters.push(scope);
        }
        if let Some(cond) = inner(&mut sub, child) {
            filters.push(cond);
        }
        self.sub_counter = sub.sub_counter;
        let from = format!("{} AS {}", self.qt(child), self.q(&child_alias));
        let mut joins = String::new();
        push_joins(&mut joins, self.dialect, &sub.joins);

        // MySQL/MariaDB can't read the table a statement writes in a subquery; a derived
        // table is materialized first, so the rows it returns are read before the write.
        let custom_on = matches!(
            child.member(&via).map(|m| &m.kind),
            Some(MemberKind::Forward {
                custom_on: Some(_),
                ..
            })
        );
        if self.dialect.is_mysql_family()
            && self.write_table.as_deref() == Some(&child.table)
            && !custom_on
        {
            let pairs = self.to_many_pairs(child, &via, cur);
            let source = format!("{from}{joins}");
            return self.materialized_any(&pairs, &alias, &child_alias, &source, &filters);
        }
        let mut wheres = vec![self.to_many_correlation(child, &via, cur, &child_alias, &alias)];
        wheres.extend(filters);
        format!(
            "EXISTS (SELECT 1 FROM {from}{joins} WHERE {})",
            wheres.join(" AND ")
        )
    }

    /// `(<parent key>) IN (SELECT * FROM (SELECT <child fk> FROM … WHERE …) AS d<n>)` — the
    /// uncorrelated form of `EXISTS` for a self-referential write, over the edge's
    /// `(child_fk, parent_pk)` pairs.
    fn materialized_any(
        &mut self,
        pairs: &[(String, String)],
        parent_alias: &str,
        child_alias: &str,
        source: &str,
        filters: &[String],
    ) -> String {
        let mut wheres: Vec<String> = pairs
            .iter()
            .map(|(fk, _)| format!("{} IS NOT NULL", self.qcol(child_alias, fk)))
            .collect();
        wheres.extend(filters.iter().cloned());
        let keys: Vec<String> = pairs
            .iter()
            .enumerate()
            .map(|(i, (fk, _))| {
                format!(
                    "{} AS {}",
                    self.qcol(child_alias, fk),
                    self.q(&format!("k{i}"))
                )
            })
            .collect();
        let parents: Vec<String> = pairs
            .iter()
            .map(|(_, pk)| self.qcol(parent_alias, pk))
            .collect();
        self.sub_counter += 1;
        let derived = format!("d{}", self.sub_counter);
        let lhs = if let [one] = parents.as_slice() {
            one.clone()
        } else {
            format!("({})", parents.join(", "))
        };
        format!(
            "{lhs} IN (SELECT * FROM (SELECT {} FROM {source} WHERE {}) AS {})",
            keys.join(", "),
            wheres.join(" AND "),
            self.q(&derived)
        )
    }
}
