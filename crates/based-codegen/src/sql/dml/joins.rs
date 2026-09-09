//! JOIN construction — forward/inverse relation joins and to-many correlation.

use super::*;

// ---------- the join-accumulating resolver --------------------------------

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

/// Render a relation's custom `on:` join condition (`on: order.user_ref =
/// user.legacy_id`) as the join `ON` SQL. Unlike a `where` predicate — resolved
/// against one model's field paths — a join condition names both joined models
/// explicitly by table (`<table>.<column>`): the FK-holding `near` model and its
/// far target. A column qualified by `near_table` resolves to `near_alias`; any
/// other qualifier is the far side, resolving to `far_alias`. Sema
/// (`resolve::check_relation_on`) has already verified every path is
/// `<table>.<column>` naming one of the two models with a real column, and that no
/// `$param` / filter / function appears — so the value arm need only handle a
/// column path or a literal.
///
/// A self-referential custom join (near and far are the same table) can't be
/// disambiguated by table name, so sema rejects it up front (`E0127`); this
/// renderer therefore only ever sees a two-model join.
fn render_join_on(
    dialect: Dialect,
    pred: &Predicate,
    near_table: &str,
    near_alias: &str,
    far_alias: &str,
) -> String {
    let col = |p: &Path| -> String {
        let table = p.segments.first().map_or("", |s| s.node.as_str());
        let column = p.segments.get(1).map_or("", |s| s.node.as_str());
        let alias = if table == near_table {
            near_alias
        } else {
            far_alias
        };
        dialect.qcol(alias, column)
    };
    let val = |v: &Value| -> String {
        match v {
            Value::Path(p) => col(p),
            Value::Lit(l) => render_lit(dialect, l),
            // Sema rejects params/functions in a join condition; guard defensively.
            _ => "NULL".to_string(),
        }
    };
    let recur = |p| render_join_on(dialect, p, near_table, near_alias, far_alias);
    match pred {
        Predicate::And(a, b) => format!("({} AND {})", recur(a), recur(b)),
        Predicate::Or(a, b) => format!("({} OR {})", recur(a), recur(b)),
        Predicate::Not(inner) => format!("NOT ({})", recur(inner)),
        Predicate::Cmp { path, op, value } => {
            let (lhs, rhs) = (col(path), val(value));
            match op {
                Op::In => format!("{lhs} IN ({rhs})"),
                Op::Has => match dialect {
                    Dialect::Postgres => format!("{lhs} @> {rhs}"),
                    _ => format!("{rhs} MEMBER OF({lhs})"),
                },
                _ => format!("{lhs} {} {rhs}", sql_op(*op)),
            }
        }
        Predicate::InList { path, values } => {
            let items: Vec<String> = values.iter().map(&val).collect();
            format!("{} IN ({})", col(path), items.join(", "))
        }
        Predicate::Bare(path) => format!("{} = {}", col(path), dialect.bool_lit(true)),
        Predicate::Raw(raw) => format!("({})", render_raw(dialect, raw, near_alias, near_table)),
        // Sema rejects a named-filter call in a join; lower to a harmless truth.
        Predicate::FilterCall { .. } => "TRUE".to_string(),
    }
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

    /// The `(fk_col, referenced_pk_col)` pairs a forward relation joins on — one pair for a
    /// single-column-key target, several (in key order) for a composite key. Owned strings,
    /// so the schema borrow is released before the join `ON` is built.
    fn fk_join_pairs(&self, mem: &RMember) -> Vec<(String, String)> {
        self.schema
            .fk_columns(mem)
            .into_iter()
            .map(|(fk, part)| (fk, part.physical_col().to_string()))
            .collect()
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
        if let Some(sd) = &tmodel.soft_delete {
            on.push_str(&format!(
                " AND {}",
                soft_pred(self.dialect, &alias, tmodel, sd)
            ));
        }
        // A joined *scoped* model rides its `@scope` into the `ON` too — the
        // exact parallel of the soft-delete injection above, so a query reaching
        // another tenant's row through a relation can't read across the scope
        // boundary. A LEFT JOIN stays a left join (the predicate is in `ON`, not
        // `WHERE`): an out-of-scope joined row simply yields NULLs.
        if let Some(scope) = self.scope_join_pred(&alias, tmodel) {
            on.push_str(&format!(" AND {scope}"));
        }
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
        if let Some(sd) = &tmodel.soft_delete {
            on.push_str(&format!(
                " AND {}",
                soft_pred(self.dialect, &alias, tmodel, sd)
            ));
        }
        // Joined-model `@scope` rides the `ON` too, same as the forward join.
        if let Some(scope) = self.scope_join_pred(&alias, tmodel) {
            on.push_str(&format!(" AND {scope}"));
        }
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
}
