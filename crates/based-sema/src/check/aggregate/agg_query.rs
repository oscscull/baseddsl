use super::*;

/// A projected non-aggregate column of an aggregate shape (`buyer = placed_by` /
/// `day = date(created_at)`): the column path it projects and the date bucket applied to
/// it — the key that must appear in `group by`.
struct GroupCol {
    path: Path,
    bucket: Option<Bucket>,
    span: Option<Span>,
}

impl GroupCol {
    fn column(path: Path) -> Self {
        let span = path.segments.last().map(|s| s.span);
        Self {
            path,
            bucket: None,
            span,
        }
    }

    /// Whether `key` groups exactly this projection.
    fn grouped_by(&self, key: &GroupKey) -> bool {
        match (key, self.bucket) {
            (GroupKey::Column(p), None) => same_path(p, &self.path),
            (GroupKey::Bucket(b), Some(bucket)) => {
                b.bucket == bucket && b.arg().is_some_and(|p| same_path(p, &self.path))
            }
            _ => false,
        }
    }

    /// The projection as written (`placed_by` / `date(created_at)`).
    fn display(&self) -> String {
        match self.bucket {
            Some(b) => format!("{}({})", b.name(), join_path(&self.path)),
            None => join_path(&self.path),
        }
    }
}

/// Summarize an aggregate shape body: the output names of all projected fields (for
/// `order`/`having` reference checks) and the non-aggregate columns (which must be
/// grouped). A raw value is opaque — a projected name that needs no grouping.
fn summarize_agg_shape(body: &[ShapeField]) -> (Vec<String>, Vec<GroupCol>) {
    let mut out_names = Vec::new();
    let mut group_cols = Vec::new();
    for f in body {
        match f {
            ShapeField::Bare(id) => {
                out_names.push(id.node.clone());
                group_cols.push(GroupCol::column(Path {
                    segments: vec![id.clone()],
                }));
            }
            ShapeField::Rename { out, value } => {
                out_names.push(out.node.clone());
                match value {
                    ShapeValue::Path(p) => group_cols.push(GroupCol::column(p.clone())),
                    // A malformed call is already reported by the bucket check.
                    ShapeValue::Bucket(b) => {
                        if let Some(p) = b.arg() {
                            group_cols.push(GroupCol {
                                path: p.clone(),
                                bucket: Some(b.bucket),
                                span: Some(b.span),
                            });
                        }
                    }
                    _ => {}
                }
            }
            // Nests are rejected on an aggregate shape; ignore here.
            ShapeField::Nest { field, .. } | ShapeField::NestRef { field, .. } => {
                out_names.push(field.node.clone());
            }
            ShapeField::Flatten { out, .. } => out_names.push(out.node.clone()),
            // Expanded away before sema (based_sema::expand_spreads).
            ShapeField::Spread { .. } => {}
        }
    }
    (out_names, group_cols)
}

/// Two paths naming the same column (segment-for-segment). Used to match a projected
/// non-aggregate column against a `group by` term.
fn same_path(a: &Path, b: &Path) -> bool {
    a.segments.len() == b.segments.len()
        && a.segments
            .iter()
            .zip(&b.segments)
            .all(|(x, y)| x.node == y.node)
}

/// The `group by` / `having` clauses of a query body (aggregate queries only). Returns
/// the group keys and the having predicate, plus whether either clause was written.
fn agg_clauses(clauses: &[Clause]) -> (Vec<&GroupKey>, Option<&Predicate>, bool) {
    let mut groups = Vec::new();
    let mut having = None;
    let mut present = false;
    for c in clauses {
        match c {
            Clause::GroupBy(cols) => {
                present = true;
                groups.extend(cols.iter());
            }
            Clause::Having(p) => {
                present = true;
                having = Some(p);
            }
            _ => {}
        }
    }
    (groups, having, present)
}

/// `group by` / `having` on a non-aggregate query is an error.
pub(crate) fn reject_agg_clauses(q: &Query, clauses: &[Clause], sink: &mut Sink) {
    for c in clauses {
        match c {
            Clause::GroupBy(cols) => {
                let span = cols.first().and_then(GroupKey::span).unwrap_or(q.span);
                sink.error_note(
                    code::AGG_CONTEXT,
                    span,
                    format!(
                        "`group by` on query `{}` needs an aggregate return shape",
                        q.name.node
                    ),
                    "add a `count()`/`sum(…)`/… field to the return shape, or drop `group by`",
                );
            }
            Clause::Having(_) => sink.error_note(
                code::AGG_CONTEXT,
                q.span,
                format!(
                    "`having` on query `{}` needs an aggregate return shape",
                    q.name.node
                ),
                "`having` filters aggregate groups — add aggregate fields, or drop it",
            ),
            _ => {}
        }
    }
}

/// Validate an aggregate query's clauses: `where` (rows, before grouping) resolves
/// normally; every non-aggregate projected column must be a `group by` column;
/// `order`/`having` name projected columns; `page` is rejected.
pub(crate) fn check_agg_query(
    q: &Query,
    clauses: &[Clause],
    mi: usize,
    body: &[ShapeField],
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let (out_names, group_cols) = summarize_agg_shape(body);
    let (group_keys, having, _present) = agg_clauses(clauses);

    // Group-by keys must resolve against the model.
    for key in &group_keys {
        match key {
            GroupKey::Column(p) => {
                if let Some(term) = resolve::resolve_path(p, mi, cx, sink) {
                    resolve::reject_opaque(&term, p, "group", sink);
                }
            }
            GroupKey::Bucket(b) => check_bucket_call(b, mi, cx, sink),
        }
    }
    // Group-by consistency: every projected non-aggregate column must be grouped.
    for gc in &group_cols {
        if !group_keys.iter().any(|key| gc.grouped_by(key)) {
            sink.error_note(
                code::AGG_GROUP_BY,
                gc.span.unwrap_or(q.span),
                format!(
                    "projected column `{}` must be a `group by` column",
                    gc.display()
                ),
                "add it to `group by`, or make it an aggregate (`count()`/`sum(…)`/…)",
            );
        }
    }

    for c in clauses {
        match c {
            Clause::Where(p) => resolve::check_predicate(p, Some(mi), cx, params, sink),
            Clause::Order(terms) => {
                for t in terms {
                    if t.path.segments.len() != 1 || !out_names.contains(&t.path.segments[0].node) {
                        let span = t.path.segments.last().map_or(q.span, |s| s.span);
                        sink.error_note(
                            code::AGG_GROUP_BY,
                            span,
                            format!(
                                "`order` on `{}` must name a projected column of the aggregate shape",
                                join_path(&t.path)
                            ),
                            "order by an aggregate alias or a group column you project",
                        );
                    }
                }
            }
            Clause::Page(_) => sink.error_note(
                code::AGG_PAGE,
                q.span,
                format!("aggregate query `{}` can't be paginated", q.name.node),
                "grouped keyset paging is unsupported — drop `page`",
            ),
            Clause::GroupBy(_) | Clause::Having(_) | Clause::Unindexed(_) | Clause::Limit(_) => {}
        }
    }

    if let Some(hp) = having {
        check_having(hp, cx.model(mi), &out_names, params, q.span, sink);
    }
}
