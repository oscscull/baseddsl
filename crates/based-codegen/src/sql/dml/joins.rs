//! Build the JOIN clause for a forward or inverse relation edge, and emit the accumulated
//! joins into a statement.

use super::*;

/// A pending JOIN. Deduped by `alias` (one per traversed path prefix), so a shape
/// and a `where` that both reach through `placed_by` share the single join.
pub(crate) struct Join {
    pub(crate) kind: &'static str, // "JOIN" | "LEFT JOIN"
    pub(crate) table: String,
    /// The joined model's `@schema("…")` namespace, so a join into a schema-qualified table
    /// names `schema.table`. `None` = default namespace.
    pub(crate) schema: Option<String>,
    pub(crate) alias: String,
    pub(crate) on: String,
}

/// Append each accumulated join to a statement (`<kind> <table> AS <alias> ON <on>`),
/// identifiers quoted for the dialect. Shared by the read side (SELECT/COUNT) and the
/// write side (`mutations`'s multi-table UPDATE/DELETE) so the two can't drift.
pub(crate) fn push_joins(s: &mut String, dialect: Dialect, joins: &[Join]) {
    for j in joins {
        s.push_str(&format!(
            "\n{} {} AS {} ON {}",
            j.kind,
            dialect.quote_table(j.schema.as_deref(), &j.table),
            dialect.quote(&j.alias),
            j.on
        ));
    }
}

/// Extend the dotted join-path prefix key (`address` + `city` -> `address.city`), the key
/// that dedups a join across every clause that reaches through the same path.
fn push_prefix(prefix: &mut String, seg: &str) {
    if !prefix.is_empty() {
        prefix.push('.');
    }
    prefix.push_str(seg);
}

impl<'a> Select<'a> {
    /// Materialize the JOIN for a to-**one** relation member `field` on `model` (rooted
    /// at `alias`/`prefix`), returning `(joined_alias, joined_prefix, target_model)` for
    /// projecting the nested sub-object's columns. `None` for a scalar (not a relation)
    /// or a to-**many** inverse edge (handled by the to-many subquery path, not here).
    /// A Forward relation is always to-one; an Inverse is
    /// to-one only when its paired forward FK (`via`) is unique on the target (a genuine
    /// one-to-one back edge), else it is a collection.
    pub(crate) fn enter_to_one(
        &mut self,
        field: &str,
        alias: &str,
        prefix: &str,
        model: &RModel,
    ) -> Option<(String, String, &'a RModel)> {
        let mem = model.member(field)?;
        let mut prefix = prefix.to_string();
        match &mem.kind {
            MemberKind::Forward { optional, .. } => {
                let (a, m) = self.join_forward(alias, model, &mut prefix, field, *optional);
                Some((a, prefix, m))
            }
            MemberKind::Inverse { target, via } => {
                let tmodel = self.schema.model(target)?;
                if !tmodel.is_unique(via) {
                    return None; // to-many collection — handled by the to-many subquery path.
                }
                let (a, m) = self.join_inverse(alias, model, &mut prefix, field, target, via);
                Some((a, prefix, m))
            }
            MemberKind::Scalar { .. } => None,
        }
    }

    /// The `(fk_col, referenced_pk_col)` pairs a forward relation joins on — one pair for a
    /// single-column-key target, several (in key order) for a composite key. Owned strings,
    /// so the schema borrow is released before the join `ON` is built.
    pub(crate) fn fk_join_pairs(&self, mem: &RMember) -> Vec<(String, String)> {
        self.schema
            .fk_columns(mem)
            .into_iter()
            .map(|(fk, part)| (fk, part.physical_col().to_string()))
            .collect()
    }

    /// Append the joined model's soft-delete tombstone + `@scope` to a built join `ON`, so a
    /// `LEFT JOIN` stays a left join (an out-of-scope / deleted joined row yields NULLs
    /// rather than dropping the outer row). The exact parallel injection both join
    /// directions share.
    fn finalize_join_on(&self, on: &mut String, alias: &str, tmodel: &RModel) {
        if let Some(sd) = &tmodel.soft_delete {
            on.push_str(&format!(
                " AND {}",
                soft_pred(self.dialect, alias, tmodel, sd)
            ));
        }
        if let Some(scope) = self.scope_join_pred(alias, tmodel) {
            on.push_str(&format!(" AND {scope}"));
        }
    }

    /// FK on this table -> JOIN target ON target.<pk> = cur.<fk>, every key part ANDed for a
    /// composite key. Optional -> LEFT JOIN.
    pub(crate) fn join_forward(
        &mut self,
        cur_alias: &str,
        cur_model: &RModel,
        prefix: &mut String,
        field: &str,
        optional: bool,
    ) -> (String, &'a RModel) {
        push_prefix(prefix, field);
        let mem = cur_model.member(field).expect("forward member resolved");
        let target = mem.kind.target().expect("forward has a target");
        let tmodel = self.schema.model(target).expect("relation target resolved");
        if let Some(a) = self.seen.get(prefix) {
            return (a.clone(), tmodel);
        }
        let alias = format!("j_{}", prefix.replace('.', "_"));
        let kind = if optional { "LEFT JOIN" } else { "JOIN" };
        // A custom `on:` relation renders its declared condition (`cur` is the
        // FK-holding near side, `tmodel` the far target); otherwise the conventional
        // `<field>_id = pk` correlation, one part per composite key column.
        let mut on = if let MemberKind::Forward {
            custom_on: Some(pred),
            ..
        } = &mem.kind
        {
            render_join_on(self.dialect, pred, &cur_model.table, cur_alias, &alias)
        } else {
            self.fk_join_pairs(mem)
                .iter()
                .map(|(fk, pk)| format!("{} = {}", self.qcol(&alias, pk), self.qcol(cur_alias, fk)))
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        self.finalize_join_on(&mut on, &alias, tmodel);
        self.record(
            kind,
            tmodel.table.clone(),
            tmodel.schema.clone(),
            alias.clone(),
            on,
            prefix,
        );
        (alias, tmodel)
    }

    /// FK on the target table -> LEFT JOIN target ON target.<via_fk> = cur.<pk>, every key
    /// part ANDed for a composite key. `cur_model` is the current (parent) model, whose
    /// primary-key column(s) the child's back-FK references.
    pub(crate) fn join_inverse(
        &mut self,
        cur_alias: &str,
        cur_model: &RModel,
        prefix: &mut String,
        field: &str,
        target: &str,
        via: &str,
    ) -> (String, &'a RModel) {
        push_prefix(prefix, field);
        let tmodel = self.schema.model(target).expect("relation target resolved");
        if let Some(a) = self.seen.get(prefix) {
            return (a.clone(), tmodel);
        }
        let alias = format!("j_{}", prefix.replace('.', "_"));
        // The forward field `via` on the target carries the FK column(s) back to us, paired
        // with our primary-key column(s) in key order — or a custom `on:` condition, whose
        // near side is the FK-holding `tmodel` and far side our `cur_model`.
        let mut on = if let Some(MemberKind::Forward {
            custom_on: Some(pred),
            ..
        }) = tmodel.member(via).map(|m| &m.kind)
        {
            render_join_on(self.dialect, pred, &tmodel.table, &alias, cur_alias)
        } else {
            let pairs: Vec<(String, String)> = match tmodel.member(via) {
                Some(m) if matches!(m.kind, MemberKind::Forward { .. }) => self.fk_join_pairs(m),
                _ => vec![(format!("{via}_id"), pk_col(cur_model))],
            };
            pairs
                .iter()
                .map(|(via_fk, cur_pk)| {
                    format!(
                        "{} = {}",
                        self.qcol(&alias, via_fk),
                        self.qcol(cur_alias, cur_pk)
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        self.finalize_join_on(&mut on, &alias, tmodel);
        self.record(
            "LEFT JOIN",
            tmodel.table.clone(),
            tmodel.schema.clone(),
            alias.clone(),
            on,
            prefix,
        );
        (alias, tmodel)
    }

    /// Record a materialized join, keyed by its path prefix so the next reach through the
    /// same path reuses it.
    pub(crate) fn record(
        &mut self,
        kind: &'static str,
        table: String,
        schema: Option<String>,
        alias: String,
        on: String,
        prefix: &str,
    ) {
        self.seen.insert(prefix.to_string(), alias.clone());
        self.joins.push(Join {
            kind,
            table,
            schema,
            alias,
            on,
        });
    }
}
