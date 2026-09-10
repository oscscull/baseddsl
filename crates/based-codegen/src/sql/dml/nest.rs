//! JSON nesting subqueries: a to-one object, a to-many correlated JSON-array aggregate, and
//! a far-side flatten. Plus the `field -> Shape` reference expansion these share.

use super::*;

/// The ORDER BY clause for a nested-array subquery's sort cascade, resolved against `model`
/// at `alias` (dotted terms join inside the subquery's own scope). `None` when no terms
/// apply, leaving element order unspecified.
fn order_clause(
    sub: &mut Select,
    terms: &[SortTerm],
    alias: &str,
    model: &RModel,
) -> Option<String> {
    let keys: Vec<String> = terms
        .iter()
        .map(|t| {
            let (a, col) = sub.resolve_from(&t.path, alias, "", model);
            let nullable = path_nullable(sub.schema, model, &t.path);
            order_by_term(&sub.qcol(&a, &col), t.dir, nullable, t.nulls, sub.dialect)
        })
        .collect();
    (!keys.is_empty()).then(|| keys.join(", "))
}

impl<'a> Select<'a> {
    /// Enter a `field -> Shape` expansion: the referenced shape's body, or `None` for
    /// an unknown name or a reference already mid-expansion (a cycle sema rejects).
    /// Every `Some` must be paired with an [`exit_shape_ref`](Self::exit_shape_ref).
    pub(crate) fn enter_shape_ref(&mut self, name: &str) -> Option<&'a [ShapeField]> {
        if self.shape_stack.contains(&name) {
            return None;
        }
        let shape = self.shapes.get(name).copied()?;
        self.shape_stack.push(shape.name.node.as_str());
        Some(&shape.body)
    }

    pub(crate) fn exit_shape_ref(&mut self) {
        self.shape_stack.pop();
    }

    /// Build the correlated-subquery expression that aggregates a to-many child edge into
    /// a JSON array of the projected element bodies (L1). The child gets a fresh
    /// `s<n>_<table>` root alias (distinct from `outer_alias`, so a self-referential edge
    /// works), the element body is projected as a per-dialect JSON object, and the
    /// subquery correlates the child's back FK to `outer_alias.id`, applying the child's
    /// soft-delete tombstone + `@scope` exactly as a join would. The array's element
    /// order follows the sort cascade for the traversal — the edge's relation `@sort`
    /// (`edge_sort`), else the child model's `@sort` — as an ORDER BY *inside* the
    /// aggregate; with neither declared the order stays unspecified.
    pub(crate) fn json_array_subquery(
        &mut self,
        body: &'a [ShapeField],
        child: &'a RModel,
        via_field: &str,
        outer_alias: &str,
        outer_model: &RModel,
        edge_sort: &[SortTerm],
    ) -> String {
        self.sub_counter += 1;
        let child_alias = format!("s{}_{}", self.sub_counter, child.table);
        // Fresh join scope for the subquery: its reaches/to-one nests accumulate joins
        // into `sub`, not the outer SELECT. The counter is threaded through so nested
        // subqueries keep minting distinct aliases.
        let mut sub = self.spawn_child(child_alias.clone());
        let elem = sub.json_object_expr(body, child, &child_alias, "");
        // Sort cascade for the traversal: relation `@sort` on the edge beats the child
        // model's `@sort`.
        let sort_terms: &[SortTerm] = if edge_sort.is_empty() {
            &child.sort
        } else {
            edge_sort
        };
        let order = order_clause(&mut sub, sort_terms, &child_alias, child);
        self.sub_counter = sub.sub_counter;

        let mut wheres: Vec<String> = vec![self.to_many_correlation(
            child,
            via_field,
            outer_model,
            &child_alias,
            outer_alias,
        )];
        if let Some(sd) = &child.soft_delete {
            wheres.push(soft_pred(self.dialect, &child_alias, child, sd));
        }
        if let Some(scope) = sub.scope_join_pred(&child_alias, child) {
            wheres.push(scope);
        }

        let mut sql = format!(
            "(SELECT {} FROM {} AS {}",
            self.dialect.json_array_agg(&elem, order.as_deref()),
            self.qt(child),
            self.q(&child_alias)
        );
        push_joins(&mut sql, self.dialect, &sub.joins);
        sql.push_str(&format!(" WHERE {})", wheres.join(" AND ")));
        sql
    }

    /// A fresh child `Select` rooted at `root_alias` with its own join scope, sharing
    /// this select's schema/dialect/filters/scope injection and the current subquery
    /// counter (so nested aliases stay globally distinct). For a to-many / flatten
    /// correlated subquery's own reaches.
    fn spawn_child(&self, root_alias: String) -> Self {
        Select {
            schema: self.schema,
            dialect: self.dialect,
            root_alias,
            joins: Vec::new(),
            seen: HashMap::new(),
            filters: self.filters.clone(),
            filter_stack: Vec::new(),
            shapes: self.shapes.clone(),
            shape_stack: self.shape_stack.clone(),
            bindings: HashMap::new(),
            inject_scope: self.inject_scope,
            scope_inject: self.scope_inject,
            sub_counter: self.sub_counter,
            bare_cols: false,
            incoming: false,
            optional_params: self.optional_params.clone(),
        }
    }

    /// Build the correlated-subquery expression for a **far-side flattening projection**
    /// (`out = edge.far { body }`): the *distinct* set of far-side rows reached through a
    /// to-many junction, the junction hidden. Two nested correlated levels:
    ///
    /// ```sql
    /// (SELECT <json-agg>(<json-object of body>) FROM <far> AS <far_alias>
    ///  WHERE <far_alias>.id IN (
    ///     SELECT <jx>.<far_fk> FROM <junction> AS <jx> [<intermediate joins>]
    ///     WHERE <jx>.<near_fk> = <outer>.id AND <junction soft-delete/@scope>)
    ///  AND <far soft-delete/@scope>)
    /// ```
    ///
    /// Iterating far rows (`FROM far`, `far.id IN (…)`) gives DISTINCT-on-PK for free —
    /// duplicate junction links collapse to one far row, so the junction's link
    /// cardinality never leaks — on every dialect (a portable `IN`, no `DISTINCT`
    /// gymnastics). The junction's own scope/soft-delete ride the inner `IN`; the far
    /// model's ride the outer `WHERE`. Element order follows the far model's `@sort`,
    /// else unspecified (portable JSON aggregation has no cross-dialect ordered form).
    /// Returns `None` on a malformed path (sema reports it).
    pub(crate) fn json_flatten_subquery(
        &mut self,
        body: &'a [ShapeField],
        path: &Path,
        outer_alias: &str,
        root: &'a RModel,
    ) -> Option<String> {
        let segs = &path.segments;
        // First hop: a to-many inverse edge into the junction.
        let (junction, near_via, _) = self.to_many_edge(&segs[0].node, root)?;
        let (inner_sql, far_model) =
            self.flatten_inner_subquery(segs, junction, &near_via, root, outer_alias)?;

        // Outer aggregation: the far rows in that set, projected + deduped by PK.
        self.sub_counter += 1;
        let far_alias = format!("s{}_{}", self.sub_counter, far_model.table);
        let mut far_sel = self.spawn_child(far_alias.clone());
        let elem = far_sel.json_object_expr(body, far_model, &far_alias, "");
        let order = order_clause(&mut far_sel, &far_model.sort, &far_alias, far_model);
        self.sub_counter = far_sel.sub_counter;

        let mut far_wheres = vec![format!(
            "{} IN ({})",
            self.qcol(&far_alias, &pk_col(far_model)),
            inner_sql
        )];
        if let Some(sd) = &far_model.soft_delete {
            far_wheres.push(soft_pred(self.dialect, &far_alias, far_model, sd));
        }
        if let Some(scope) = far_sel.scope_join_pred(&far_alias, far_model) {
            far_wheres.push(scope);
        }
        let mut sql = format!(
            "(SELECT {} FROM {} AS {}",
            self.dialect.json_array_agg(&elem, order.as_deref()),
            self.qt(far_model),
            self.q(&far_alias)
        );
        push_joins(&mut sql, self.dialect, &far_sel.joins);
        sql.push_str(&format!(" WHERE {})", far_wheres.join(" AND ")));
        Some(sql)
    }

    /// The inner correlated subquery of a flatten: walk the forward hops from the junction,
    /// joining intermediate models, and select the far FK off the last hop — correlated to
    /// the outer row and carrying the junction's own soft-delete/`@scope`. Returns the inner
    /// `SELECT … ` text and the resolved far model.
    fn flatten_inner_subquery(
        &mut self,
        segs: &[Spanned<String>],
        junction: &'a RModel,
        near_via: &str,
        root: &'a RModel,
        outer_alias: &str,
    ) -> Option<(String, &'a RModel)> {
        self.sub_counter += 1;
        let jx_alias = format!("s{}_{}", self.sub_counter, junction.table);
        let mut inner = self.spawn_child(jx_alias.clone());
        let mut cur_alias = jx_alias.clone();
        let mut cur_model = junction;
        let mut prefix = String::new();
        let last = segs.len() - 1;
        let mut far_fk = None;
        let mut far_model: Option<&'a RModel> = None;
        for (i, seg) in segs[1..].iter().enumerate() {
            let idx = i + 1;
            match cur_model.member(&seg.node).map(|m| &m.kind) {
                Some(MemberKind::Forward {
                    target,
                    fk_col,
                    optional,
                    ..
                }) => {
                    if idx == last {
                        far_fk = Some(fk_col.clone());
                        far_model = self.schema.model(target);
                    } else {
                        let (a, m) = inner.join_forward(
                            &cur_alias,
                            cur_model,
                            &mut prefix,
                            &seg.node,
                            *optional,
                        );
                        cur_alias = a;
                        cur_model = m;
                    }
                }
                _ => return None,
            }
        }
        let (far_fk, far_model) = (far_fk?, far_model?);
        self.sub_counter = inner.sub_counter;

        let mut inner_wheres: Vec<String> =
            vec![self.to_many_correlation(junction, near_via, root, &jx_alias, outer_alias)];
        if let Some(sd) = &junction.soft_delete {
            inner_wheres.push(soft_pred(self.dialect, &jx_alias, junction, sd));
        }
        if let Some(scope) = inner.scope_join_pred(&jx_alias, junction) {
            inner_wheres.push(scope);
        }
        let mut inner_sql = format!(
            "SELECT {} FROM {} AS {}",
            self.qcol(&cur_alias, &far_fk),
            self.qt(junction),
            self.q(&jx_alias)
        );
        push_joins(&mut inner_sql, self.dialect, &inner.joins);
        inner_sql.push_str(&format!(" WHERE {}", inner_wheres.join(" AND ")));
        Some((inner_sql, far_model))
    }

    /// Build a per-dialect JSON-object expression (`json_object('k', v, …)`) for a shape
    /// body over `model` at `alias`/`prefix` — one element of a to-many nested array. A
    /// bare field / reach becomes a `'key', <col>` pair; a to-one nest a nested JSON
    /// object; a to-many nest a nested correlated-subquery array. Reaches and to-one nests
    /// materialize their joins into this (sub-)`Select`'s join scope.
    fn json_object_expr(
        &mut self,
        body: &'a [ShapeField],
        model: &'a RModel,
        alias: &str,
        prefix: &str,
    ) -> String {
        let mut pairs: Vec<String> = Vec::new();
        for f in body {
            match f {
                ShapeField::Bare(id) => {
                    let path = single(&id.node);
                    let (a, col) = self.resolve_from(&path, alias, prefix, model);
                    let expr = self.json_scalar(&a, &col, &path, model);
                    pairs.push(format!("'{}', {expr}", id.node));
                }
                ShapeField::Rename { out, value } => match value {
                    ShapeValue::Path(p) => {
                        let (a, col) = self.resolve_from(p, alias, prefix, model);
                        let expr = self.json_scalar(&a, &col, p, model);
                        pairs.push(format!("'{}', {expr}", out.node));
                    }
                    ShapeValue::Raw(raw) => {
                        pairs.push(format!(
                            "'{}', ({})",
                            out.node,
                            render_raw(self.dialect, raw, alias, &model.table)
                        ));
                    }
                    // An aggregate shape can't be nested (sema `E0245`); handle the arm
                    // defensively so the match is total.
                    ShapeValue::Agg(agg) => {
                        let d = self.dialect;
                        let expr = agg_sql(self, model, alias, prefix, agg, d, true);
                        pairs.push(format!("'{}', {expr}", out.node));
                    }
                    // A per-row derived scalar inside a to-many element body.
                    ShapeValue::Computed(expr) => {
                        let sql = self.shape_expr(expr, alias, prefix, model);
                        pairs.push(format!("'{}', {sql}", out.node));
                    }
                },
                ShapeField::Nest { field, body } => {
                    self.json_nest_pair(field, body, model, alias, prefix, &mut pairs);
                }
                ShapeField::NestRef { field, shape } => {
                    if let Some(body) = self.enter_shape_ref(&shape.node) {
                        self.json_nest_pair(field, body, model, alias, prefix, &mut pairs);
                        self.exit_shape_ref();
                    }
                }
                // A flatten nested inside a to-many element: its own correlated
                // subquery, keyed off this element's alias.
                ShapeField::Flatten { out, path, body } => {
                    if let Some(arr) = self.json_flatten_subquery(body, path, alias, model) {
                        pairs.push(format!("'{}', {}", out.node, arr));
                    }
                }
                ShapeField::Spread { .. } => unreachable!("spreads expanded before codegen"),
            }
        }
        format!("{}({})", self.dialect.json_object_fn(), pairs.join(", "))
    }

    /// One relation nest inside a JSON element body: a to-one edge becomes a nested
    /// JSON object, a to-many edge a nested correlated-subquery array.
    fn json_nest_pair(
        &mut self,
        field: &Ident,
        body: &'a [ShapeField],
        model: &'a RModel,
        alias: &str,
        prefix: &str,
        pairs: &mut Vec<String>,
    ) {
        if let Some((child_alias, child_prefix, child_model)) =
            self.enter_to_one(&field.node, alias, prefix, model)
        {
            let nested = self.json_object_expr(body, child_model, &child_alias, &child_prefix);
            // An absent LEFT-JOINed row must surface as JSON null, not an object of
            // nulls — probe the child's `id` (never NULL on a matched row).
            let nested = if to_one_absent_possible(model, &field.node) {
                format!(
                    "CASE WHEN {} IS NULL THEN NULL ELSE {nested} END",
                    self.qcol(&child_alias, &pk_col(child_model))
                )
            } else {
                nested
            };
            pairs.push(format!("'{}', {}", field.node, nested));
        } else if let Some((child_model, via_field, edge_sort)) =
            self.to_many_edge(&field.node, model)
        {
            let arr =
                self.json_array_subquery(body, child_model, &via_field, alias, model, edge_sort);
            pairs.push(format!("'{}', {}", field.node, arr));
        }
    }
}
