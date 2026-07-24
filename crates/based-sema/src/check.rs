//! Shape / query / mutation / filter checks, including the four query inferences:
//! verb, param type, filter, and target model.

use based_ast::*;

use crate::ir::*;
use crate::resolve::{self, Cx};
use std::collections::HashSet;

// ---------- shapes ---------------------------------------------------------

pub fn check_shape(s: &Shape, cx: &Cx, sink: &mut Sink) -> Option<RShape> {
    let Some(mi) = cx.find(&s.from.node) else {
        sink.error(
            code::UNKNOWN_MODEL,
            s.from.span,
            format!(
                "shape `{}` is from unknown model `{}`",
                s.name.node, s.from.node
            ),
        );
        return None;
    };
    let mut stack = vec![s.name.node.clone()];
    check_shape_body(&s.body, mi, cx, &mut stack, sink);
    // An aggregate shape projects groups, not rows: it must be flat (no relation nest or
    // reference — a group has no sub-objects).
    if is_agg_shape(&s.body) {
        for f in &s.body {
            if let ShapeField::Nest { field, .. }
            | ShapeField::NestRef { field, .. }
            | ShapeField::Flatten { out: field, .. } = f
            {
                sink.error_note(
                    code::AGG_COMPOSE,
                    field.span,
                    format!("aggregate shape `{}` nests `{}`", s.name.node, field.node),
                    "an aggregate shape is flat — project columns and aggregates, not sub-objects",
                );
            }
        }
    }
    Some(RShape {
        name: s.name.node.clone(),
        from: s.from.node.clone(),
        span: s.span,
    })
}

/// `stack` is the chain of named shapes currently being expanded (the declaring
/// shape at the bottom), so a `field -> Shape` reference that closes back onto it
/// is an `E0134` error instead of infinite recursion.
fn check_shape_body(
    fields: &[ShapeField],
    mi: usize,
    cx: &Cx,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) {
    for f in fields {
        match f {
            ShapeField::Bare(id) => match cx.model(mi).member(&id.node).map(|m| &m.kind) {
                Some(MemberKind::Scalar { .. }) => {}
                Some(_) => sink.error(
                    code::SHAPE_BARE_RELATION,
                    id.span,
                    format!(
                        "relation `{}` can't be projected bare; nest it (`{} {{ … }}`) or reach a column with `=`",
                        id.node, id.node
                    ),
                ),
                None => unknown_field(cx, mi, id, sink),
            },
            // A rename reaches a column via a path, computes one with raw SQL (a leaf
            // trapdoor — shapes have no params, so raw is left unchecked), or aggregates
            // a column (`= count()` / `= sum(total)`).
            ShapeField::Rename { value, .. } => match value {
                ShapeValue::Path(p) => {
                    resolve::resolve_path(p, mi, cx, sink);
                }
                ShapeValue::Raw(_) => {}
                ShapeValue::Agg(agg) => check_agg_call(agg, mi, cx, sink),
            },
            ShapeField::Nest { field, body } => {
                if let Some(ti) = nest_target(field, mi, cx, sink).and_then(|t| cx.find(t)) {
                    check_shape_body(body, ti, cx, stack, sink);
                }
            }
            // `field -> Shape`: nest a relation, projected by a named shape. The
            // reference is a pure body expansion, so the referenced shape's own decl
            // check covers its fields; here we resolve the relation, require the
            // shape's model to equal the relation target, and guard against cycles.
            ShapeField::NestRef { field, shape } => {
                if let Some(target) = nest_target(field, mi, cx, sink) {
                    check_nest_ref(shape, field, target, cx, stack, sink);
                }
            }
            // `out = path { body }`: flatten a to-many path through a junction to the
            // far side (the junction is hidden). The path resolves as a to-many inverse
            // hop then forward hops; the body projects the far model.
            ShapeField::Flatten { path, body, .. } => {
                if let Some(far) = check_flatten_path(path, mi, cx, sink) {
                    if cx.model(far).no_id {
                        let span = path.segments.last().map_or(path.segments[0].span, |s| s.span);
                        sink.error_note(
                            code::FLATTEN_KEYLESS,
                            span,
                            format!("`{}` is a keyless (`@no_id`) model", cx.model(far).name),
                            "a flattening projection returns a distinct *set* of far rows — it needs a primary key to dedup on",
                        );
                    }
                    check_shape_body(body, far, cx, stack, sink);
                }
            }
        }
    }
}

/// The model a nested field points at: its member must exist and be a relation.
/// Reports the missing-field / not-a-relation case and returns `None`.
fn nest_target<'a>(field: &Ident, mi: usize, cx: &'a Cx, sink: &mut Sink) -> Option<&'a str> {
    match cx.model(mi).member(&field.node).map(|m| &m.kind) {
        Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
            Some(target)
        }
        Some(MemberKind::Scalar { .. }) => {
            sink.error(
                code::SHAPE_NEST_SCALAR,
                field.span,
                format!(
                    "`{}` is a column, not a relation, so it can't be nested",
                    field.node
                ),
            );
            None
        }
        None => {
            unknown_field(cx, mi, field, sink);
            None
        }
    }
}

/// The `field -> Shape` half of a nest-by-reference: the named shape must exist, project
/// the relation's target model, and not be an aggregate; the reference must not cycle.
fn check_nest_ref(
    shape: &Ident,
    field: &Ident,
    target: &str,
    cx: &Cx,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) {
    match cx.shapes.get(&shape.node) {
        Some(from) if from != target => sink.error(
            code::SHAPE_REF_MODEL,
            shape.span,
            format!(
                "shape `{}` projects `{from}`, but `{}` relates to `{target}`",
                shape.node, field.node
            ),
        ),
        Some(_) => {
            if cx
                .shape_bodies
                .get(&shape.node)
                .is_some_and(|b| is_agg_shape(b))
            {
                sink.error_note(
                    code::AGG_COMPOSE,
                    shape.span,
                    format!("`{}` is an aggregate shape", shape.node),
                    "an aggregate shape is a group, not a row — it can't be nested",
                );
            }
            check_ref_cycle(&shape.node, shape.span, cx, stack, sink);
        }
        None => sink.error(
            code::SHAPE_REF_UNKNOWN,
            shape.span,
            format!("`-> {}` names no declared shape", shape.node),
        ),
    }
}

/// Validate a flatten path (`enrollments.course`): the first segment must be a
/// to-**many** inverse edge (into the junction), and each later segment a forward
/// edge to the next model. Returns the far model (the last segment's target) on a
/// clean path, else reports the offending segment and returns `None`.
fn check_flatten_path(path: &Path, mi: usize, cx: &Cx, sink: &mut Sink) -> Option<usize> {
    let segs = &path.segments;
    let first = &segs[0];
    if segs.len() < 2 {
        sink.error_note(
            code::FLATTEN_SEGMENT,
            first.span,
            format!("`{}` has no forward hop to a far side", first.node),
            "a flattening projection skips a junction: `edge.far { … }` (a to-many edge, then a forward edge)",
        );
        return None;
    }
    let mut cur = match cx.model(mi).member(&first.node).map(|m| &m.kind) {
        Some(MemberKind::Inverse { target, via }) => {
            let ti = cx.find(target)?;
            if cx.model(ti).is_unique(via) {
                sink.error(
                    code::FLATTEN_NOT_TOMANY,
                    first.span,
                    format!(
                        "`{}` is a to-one edge; a flattening projection skips a to-*many* junction",
                        first.node
                    ),
                );
                return None;
            }
            ti
        }
        Some(_) => {
            sink.error(
                code::FLATTEN_NOT_TOMANY,
                first.span,
                format!(
                    "`{}` must be a to-many edge (into the junction) to flatten through it",
                    first.node
                ),
            );
            return None;
        }
        None => {
            unknown_field(cx, mi, first, sink);
            return None;
        }
    };
    for seg in &segs[1..] {
        match cx.model(cur).member(&seg.node).map(|m| &m.kind) {
            Some(MemberKind::Forward { target, .. }) => cur = cx.find(target)?,
            Some(_) => {
                sink.error(
                    code::FLATTEN_SEGMENT,
                    seg.span,
                    format!("`{}` must be a forward relation to the far model", seg.node),
                );
                return None;
            }
            None => {
                unknown_field(cx, cur, seg, sink);
                return None;
            }
        }
    }
    Some(cur)
}

/// Follow a `-> Shape` reference for cycle detection only: a shape that transitively
/// nests itself by reference would expand forever, so it is an error (`E0134`),
/// reported at the reference that closes the cycle.
fn check_ref_cycle(shape: &str, at: Span, cx: &Cx, stack: &mut Vec<String>, sink: &mut Sink) {
    if stack.iter().any(|s| s == shape) {
        sink.error(
            code::SHAPE_REF_CYCLE,
            at,
            format!(
                "shape reference cycle: `{}` -> `{shape}`",
                stack.join("` -> `")
            ),
        );
        return;
    }
    stack.push(shape.to_string());
    if let Some(body) = cx.shape_bodies.get(shape) {
        walk_body_refs(body, cx, stack, sink);
    }
    stack.pop();
}

/// Walk a shape body's nest structure, following each `-> Shape` reference (the
/// referenced fields themselves are checked at their own decl).
fn walk_body_refs(fields: &[ShapeField], cx: &Cx, stack: &mut Vec<String>, sink: &mut Sink) {
    for f in fields {
        match f {
            ShapeField::Nest { body, .. } | ShapeField::Flatten { body, .. } => {
                walk_body_refs(body, cx, stack, sink);
            }
            ShapeField::NestRef { shape, .. } => {
                check_ref_cycle(&shape.node, shape.span, cx, stack, sink);
            }
            ShapeField::Bare(_) | ShapeField::Rename { .. } => {}
        }
    }
}

// ---------- aggregate shapes ----------------------------------------------

/// True when a shape body carries a top-level aggregate field (`= count()` / `= sum(…)`),
/// making it an aggregate shape (shapes.md) — a projection over groups, paired with a
/// query's `group by` / `having`.
pub fn is_agg_shape(body: &[ShapeField]) -> bool {
    body.iter().any(|f| {
        matches!(
            f,
            ShapeField::Rename {
                value: ShapeValue::Agg(_),
                ..
            }
        )
    })
}

/// Validate one aggregate call against its shape's model: the function must be known
/// (`E0240`), its argument arity must match (`count()` takes none, the rest one — `E0240`),
/// and the aggregated column must be an eligible type (`E0241`).
fn check_agg_call(agg: &AggCall, mi: usize, cx: &Cx, sink: &mut Sink) {
    let func = agg.func.node.as_str();
    if !KNOWN_AGGS.contains(&func) {
        sink.error(
            code::AGG_CALL,
            agg.func.span,
            format!(
                "unknown aggregate `{func}` (expected one of: {})",
                KNOWN_AGGS.join(", ")
            ),
        );
        return;
    }
    match (func, &agg.arg) {
        ("count", Some(_)) => sink.error_note(
            code::AGG_CALL,
            agg.span,
            "`count` takes no argument".to_string(),
            "`count()` counts rows in the group",
        ),
        ("count", None) => {}
        (_, None) => sink.error(
            code::AGG_CALL,
            agg.span,
            format!("`{func}` needs one column argument, e.g. `{func}(total)`"),
        ),
        (_, Some(arg)) => {
            if let Some(term) = resolve::resolve_path(arg, mi, cx, sink) {
                if resolve::reject_opaque(&term, arg, "aggregate", sink) {
                    return;
                }
                let is_enum = cx.terminal_enum(arg, mi).is_some();
                if let Some(reason) = resolve::agg_operand_reason(func, &term, is_enum) {
                    let span = arg.segments.last().map_or(agg.span, |s| s.span);
                    sink.error(code::AGG_OPERAND, span, reason);
                }
            }
        }
    }
}

/// A projected non-aggregate column of an aggregate shape (`buyer = placed_by`): its
/// output alias and the column path it projects — the path that must appear in `group by`.
struct GroupCol {
    path: Path,
}

/// Summarize an aggregate shape body: the output names of all projected fields (for
/// `order`/`having` reference checks) and the non-aggregate columns (which must be
/// grouped). A raw value is opaque — a projected name, but not a required group column.
fn summarize_agg_shape(body: &[ShapeField]) -> (Vec<String>, Vec<GroupCol>) {
    let mut out_names = Vec::new();
    let mut group_cols = Vec::new();
    for f in body {
        match f {
            ShapeField::Bare(id) => {
                out_names.push(id.node.clone());
                group_cols.push(GroupCol {
                    path: Path {
                        segments: vec![id.clone()],
                    },
                });
            }
            ShapeField::Rename { out, value } => {
                out_names.push(out.node.clone());
                if let ShapeValue::Path(p) = value {
                    group_cols.push(GroupCol { path: p.clone() });
                }
            }
            // Nests are rejected on an aggregate shape (E0245); ignore here.
            ShapeField::Nest { field, .. } | ShapeField::NestRef { field, .. } => {
                out_names.push(field.node.clone());
            }
            ShapeField::Flatten { out, .. } => out_names.push(out.node.clone()),
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
/// the group paths and the having predicate, plus whether either clause was written.
fn agg_clauses(clauses: &[Clause]) -> (Vec<&Path>, Option<&Predicate>, bool) {
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

// ---------- queries --------------------------------------------------------

struct Resolved {
    model: String,
    shape: Option<String>,
}

pub fn check_query(q: &Query, cx: &Cx, sink: &mut Sink) -> Option<RQuery> {
    // `-> ok` acknowledges a destructive mutation; a query returns data (E0222).
    if q.ret.ack {
        sink.error_note(
            code::ACK_QUERY,
            q.ret.ty.span,
            format!("query `{}` cannot return `ok`", q.name.node),
            "a query returns data — declare a shape or model; `-> ok` is for destructive mutations",
        );
        return None;
    }
    let params: Vec<String> = q.params.iter().map(|p| p.name.node.clone()).collect();
    let body_model = match &q.body {
        QueryBody::Block(s) => Some(s.model.node.as_str()),
        _ => None,
    };
    let ret = resolve_return(&q.ret, body_model, cx, sink)?;
    let ti = cx.find(&ret.model)?;

    let verb = query_verb(q, &ret.model, sink);

    // Bare/inline queries map each param onto a same-named column (the filter);
    // block and raw queries reference params via `$`, so no same-name mapping is
    // required.
    let infer = matches!(q.body, QueryBody::Bare | QueryBody::Inline(_));
    for p in &q.params {
        check_param(p, ti, infer, cx, sink);
    }

    // An aggregate return shape turns the query into an aggregate query: `group by` /
    // `having` become legal (and required for consistency), and the `get`/sort/pagination
    // rules change (queries.md).
    let agg_body: Option<&[ShapeField]> = ret
        .shape
        .as_deref()
        .and_then(|n| cx.shape_bodies.get(n).copied())
        .filter(|b| is_agg_shape(b));

    let shape = QueryShape {
        verb,
        raw: matches!(q.body, QueryBody::Raw(_)),
        agg: agg_body.is_some(),
        has_order: check_query_body(q, ti, agg_body, cx, &params, sink),
        paginated: matches!(&q.body, QueryBody::Inline(cs) | QueryBody::Block(Statement{clauses: cs, ..}) if cs.iter().any(|c| matches!(c, Clause::Page(_)))),
    };
    check_query_envelope(q, ti, &shape, cx, sink);

    // Scope acknowledgement: a callable touching a scoped
    // model must name it (`scoped …`) or opt out (`unscoped(…)`) — E0182/E0183/E0185.
    let touched = crate::scope::touched_query(q, ti, cx);
    crate::scope::check_ack(
        q.scoped.as_ref(),
        q.unscoped.is_some(),
        &touched,
        cx,
        q.span,
        sink,
    );

    // `unscoped` on a query with no `@scope` to opt out of is stale (W0106) — the twin
    // of W0105 for `unindexed`. Points the author at a no-op token to drop.
    if let Some(u) = &q.unscoped {
        if touched.is_empty() {
            sink.warn_note(
                code::STALE_UNSCOPED,
                u.span,
                format!(
                    "`unscoped` on query `{}` has no scope to opt out of",
                    q.name.node
                ),
                "drop it, or add `@scope Name` to a touched model",
            );
        }
    }

    // The shard key is the target model's `@scope` owner field  — the field the
    // request routes on — but a `unscoped` query  deliberately reads across scopes,
    // so it has no single owning shard and must route by an explicit key instead.
    let shard_key = if q.unscoped.is_some() {
        None
    } else {
        cx.model(ti).shard_key_ctx_field()
    };

    // The alternative this query injects per touched scoped model  — threaded to
    // codegen so a callable naming one `@scope` alternative filters differently from one
    // naming another. Single-alternative models resolve to the same terms as before.
    // The `$ctx` requirement derives from the same choice, so the ctx bag always
    // carries exactly the fields the injected `:ctx_<field>` binds read.
    let scope_inject =
        crate::scope::resolve_inject(q.scoped.as_ref(), q.unscoped.is_some(), &touched, cx);
    let scope_reqs =
        crate::scope::inject_ctx_reqs(q.scoped.as_ref(), q.unscoped.is_some(), &touched, cx);
    let ctx_requires = crate::ctx::collect_query(q, ti, cx, scope_reqs);

    Some(RQuery {
        name: q.name.node.clone(),
        span: q.span,
        target: ret.model,
        verb,
        many: q.ret.many || q.ret.stream,
        stream: q.ret.stream,
        ret_shape: ret.shape,
        paginated: shape.paginated,
        ctx_requires,
        shard_key,
        scope_inject,
    })
}

/// What the body turned out to be — the facts the envelope lints (keying, sort
/// determinism, stream/page exclusivity) are judged against.
struct QueryShape {
    verb: Verb,
    raw: bool,
    agg: bool,
    /// The body carries its own `order` clause.
    has_order: bool,
    paginated: bool,
}

/// The query's verb: explicit in a block body, else inferred from return cardinality.
/// Reports a block statement that reads a model the return type doesn't project.
fn query_verb(q: &Query, ret_model: &str, sink: &mut Sink) -> Verb {
    match &q.body {
        QueryBody::Block(s) => {
            if s.model.node != ret_model {
                sink.error(
                    code::RETURN_MODEL_MISMATCH,
                    s.model.span,
                    format!(
                        "statement reads `{}` but the return type is from `{ret_model}`",
                        s.model.node
                    ),
                );
            }
            s.verb
        }
        _ if q.ret.many || q.ret.stream => Verb::List,
        _ => Verb::Get,
    }
}

/// Check the body's clauses against the target model. Returns whether the body carries
/// its own `order` clause.
fn check_query_body(
    q: &Query,
    ti: usize,
    agg_body: Option<&[ShapeField]>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) -> bool {
    let (clauses, mi) = match &q.body {
        QueryBody::Bare => return false,
        QueryBody::Raw(raw) => {
            check_raw_query(q, raw, ti, cx, params, sink);
            return false;
        }
        QueryBody::Inline(clauses) => (clauses.as_slice(), ti),
        QueryBody::Block(s) => (s.clauses.as_slice(), cx.find(&s.model.node).unwrap_or(ti)),
    };
    if let Some(body) = agg_body {
        check_agg_query(q, clauses, mi, body, cx, params, sink);
        false
    } else {
        reject_agg_clauses(q, clauses, sink);
        check_clauses(clauses, mi, cx, params, sink)
    }
}

/// The result-envelope rules, judged once the body is known: a scalar `get` must be
/// keyed, a `list` must sort deterministically, `stream` and `page` are exclusive, and a
/// keyset page over a keyless model needs a unique sort key.
fn check_query_envelope(q: &Query, ti: usize, shape: &QueryShape, cx: &Cx, sink: &mut Sink) {
    let engine_built = !shape.raw && !shape.agg;

    // A stream is a list delivered incrementally: a `get` body is a cardinality
    // mismatch (E0200).
    if q.ret.stream && shape.verb == Verb::Get {
        sink.error_note(
            code::STREAM_GET,
            q.ret.ty.span,
            format!(
                "stream query `{}` uses `get` — a stream is many rows",
                q.name.node
            ),
            "use `list`, or drop `stream` for a scalar return",
        );
    }
    if shape.verb == Verb::Get && !q.ret.stream && engine_built && !get_is_keyed(q, ti, cx) {
        sink.error_note(
            code::GET_NOT_UNIQUE,
            q.span,
            format!(
                "`get` query `{}` is not keyed on a unique field",
                q.name.node
            ),
            "a scalar `get` needs an equality on `id`, a `(unique)` column, or a unique index",
        );
    }

    // A page is a bounded chunk + a re-entry cursor; a stream is one unbounded
    // forward pass — the envelopes contradict (E0201).
    if q.ret.stream && shape.paginated {
        sink.error_note(
            code::STREAM_PAGE,
            q.span,
            format!("stream query `{}` declares `page`", q.name.node),
            "paginate for random access, stream for the full pass — drop one",
        );
    }

    // Nondeterministic-order lint: a `list` with no sort at any tier.
    if shape.verb == Verb::List && engine_built && !shape.has_order && cx.model(ti).sort.is_empty()
    {
        sink.warn(
            code::NONDET_SORT,
            q.span,
            format!(
                "`list` query `{}` has no sort — results are nondeterministic; add `order (…)` or a model `@sort`",
                q.name.node
            ),
        );
    }

    // A keyless model has no `id` tiebreaker, so a keyset page (non-offset `page`) needs
    // a sort that is itself a total order — its effective sort must include a local
    // `(unique)` column, else the minted cursor could drop or repeat rows (E0263).
    let keyset = page_clause(&q.body).is_some_and(|p| !p.offset);
    let m = cx.model(ti);
    if m.no_id && engine_built && keyset {
        let deterministic = effective_sort(&q.body, m)
            .iter()
            .any(|t| t.path.segments.len() == 1 && m.is_unique(&t.path.segments[0].node));
        if !deterministic {
            sink.error_note(
                code::KEYLESS_KEYSET,
                q.span,
                format!("keyset `page` on keyless `{}` has no unique sort key", m.name),
                "a `@no_id` model has no `id` tiebreaker — `order (…)` on a `(unique)` column, or `page (…) offset`",
            );
        }
    }
}

/// Resolve a return type to its underlying model. A shape resolves via its `from`;
/// a bare model resolves to itself; `full` needs a block body naming the model.
fn resolve_return(
    ret: &RetType,
    body_model: Option<&str>,
    cx: &Cx,
    sink: &mut Sink,
) -> Option<Resolved> {
    let name = ret.ty.node.as_str();
    if name == "full" {
        return match body_model {
            Some(m) if cx.find(m).is_some() => Some(Resolved {
                model: m.to_string(),
                shape: Some("full".to_string()),
            }),
            Some(m) => {
                sink.error(
                    code::UNKNOWN_MODEL,
                    ret.ty.span,
                    format!("unknown model `{m}`"),
                );
                None
            }
            None => {
                sink.error(
                    code::FULL_NEEDS_MODEL,
                    ret.ty.span,
                    "`full` return needs a block body that names the model",
                );
                None
            }
        };
    }
    if let Some(from) = cx.shapes.get(name) {
        return Some(Resolved {
            model: from.clone(),
            shape: Some(name.to_string()),
        });
    }
    if cx.find(name).is_some() {
        return Some(Resolved {
            model: name.to_string(),
            shape: None,
        });
    }
    sink.error(
        code::UNKNOWN_RETURN,
        ret.ty.span,
        format!("unknown return type `{name}` (not a declared shape or model)"),
    );
    None
}

/// Validate a param's binding + default against the target model. When `infer` is
/// set (bare/inline query), an unbound param must name a same-named column.
fn check_param(p: &Param, ti: usize, infer: bool, cx: &Cx, sink: &mut Sink) {
    let m = cx.model(ti);
    // The column/edge this param maps onto — its type is what an explicit
    // annotation must agree with . `None` when the mapping is unresolved
    // (error already reported) or the param isn't column-mapped.
    let mapped: Option<resolve::Mapped> = match &p.binding {
        Some(ParamBinding::Edge(edge)) => match m.member(&edge.node).map(|mm| &mm.kind) {
            Some(k) if k.is_relation() => Some(resolve::Mapped::Relation(k.target().unwrap())),
            Some(_) => {
                sink.error(
                    code::BINDING_EDGE,
                    edge.span,
                    format!(
                        "`{}` is a column, not a relation, so a param can't bind via it",
                        edge.node
                    ),
                );
                None
            }
            None => {
                unknown_field(cx, ti, edge, sink);
                None
            }
        },
        Some(ParamBinding::ColOp { col, .. }) => mapped_member(m, &col.node, cx, ti, col, sink),
        // Bare/inline queries map an unbound param onto a same-named column; block
        // queries reference params via `$`, so a bare param maps to nothing.
        None if infer => match m.member(&p.name.node).map(|mm| &mm.kind) {
            Some(MemberKind::Scalar { ty, .. }) => Some(resolve::Mapped::Scalar(*ty)),
            Some(k @ (MemberKind::Forward { .. } | MemberKind::Inverse { .. })) => {
                Some(resolve::Mapped::Relation(k.target().unwrap()))
            }
            None => {
                sink.error(
                    code::UNKNOWN_FIELD,
                    p.name.span,
                    format!(
                        "param `{}` maps to a same-named column, but `{}` has none",
                        p.name.node, m.name
                    ),
                );
                None
            }
        },
        None => None,
    };

    if let (Some(ann), Some(mapped)) = (&p.ty, mapped) {
        resolve::check_param_type(ann, mapped, sink);
    }
    if let Some(d) = &p.default {
        resolve::check_default(d, sink);
    }
}

/// Check a whole-query raw body (raw.md's third level). The engine keeps only
/// param-binding and shape-typed results through this hatch, so everything it cannot
/// deliver is rejected loudly here rather than silently dropped: params must be typed
/// bind values (no column to infer a type from, no engine-built WHERE for a binding
/// to ride), `$ctx` has no type source, `scoped` would promise an injection that
/// never happens, streaming and nested shapes lean on engine-built SQL. Soft-delete
/// is the one gap that stays legal — linted (`W0102`), never silent.
fn check_raw_query(
    q: &Query,
    raw: &RawSql,
    ti: usize,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    check_raw_query_params(q, raw, params, sink);
    if q.ret.stream {
        sink.error_note(
            code::RAW_QUERY_STREAM,
            q.ret.ty.span,
            format!("raw-bodied query `{}` can't `stream`", q.name.node),
            "collect with `-> Shape[]`, or write an engine-built `list` body",
        );
    }
    if let Some(s) = &q.scoped {
        sink.error_note(
            code::RAW_QUERY_SCOPED,
            s.span,
            format!(
                "`scoped` on raw-bodied query `{}` — the engine can't inject a scope predicate into raw SQL",
                q.name.node
            ),
            "write the scope filter in the SQL yourself and mark the query `unscoped(\"…\")`",
        );
    }
    // The declared shape types the result columns by name; a nested sub-object
    // depends on engine-built projections (join aliases / JSON aggregation) that a
    // raw statement does not get.
    if let Some(body) = cx.shape_bodies.get(&q.ret.ty.node) {
        if body.iter().any(|f| {
            matches!(
                f,
                ShapeField::Nest { .. } | ShapeField::NestRef { .. } | ShapeField::Flatten { .. }
            )
        }) {
            sink.error_note(
                code::RAW_QUERY_NEST,
                q.ret.ty.span,
                format!(
                    "raw-bodied query `{}` returns shape `{}`, which nests a sub-object",
                    q.name.node, q.ret.ty.node
                ),
                "a raw body can't build nested projections — return a flat shape",
            );
        }
    }
    check_raw_soft_delete_gap(raw, ti, cx, sink);
}

/// A raw body's params must be typed bind values: there is no column to infer a type from,
/// and no engine-built WHERE for a binding to ride. `${ctx.…}` has no type source at all.
fn check_raw_query_params(q: &Query, raw: &RawSql, params: &[String], sink: &mut Sink) {
    for p in &q.params {
        if p.ty.is_none() {
            sink.error_note(
                code::RAW_QUERY_PARAM,
                p.name.span,
                format!(
                    "param `{}` of raw-bodied query `{}` needs a type annotation",
                    p.name.node, q.name.node
                ),
                "a raw body gives no column to infer the type from",
            );
        }
        if p.binding.is_some() {
            sink.error_note(
                code::RAW_QUERY_PARAM,
                p.name.span,
                format!(
                    "param `{}` of raw-bodied query `{}` can't carry a binding",
                    p.name.node, q.name.node
                ),
                "the raw SQL is the whole filter — reference the param as `${…}` inside it",
            );
        }
    }
    for part in &raw.parts {
        if let RawPart::Param(pr) = part {
            if pr.name.node == "ctx" {
                sink.error_note(
                    code::RAW_QUERY_CTX,
                    pr.name.span,
                    "`${ctx.…}` in a raw query body has no type source",
                    "declare a typed param and pass the context value through it",
                );
            } else {
                resolve::check_param_ref(pr, params, sink);
            }
        }
    }
}

/// The engine can't inject a tombstone filter into SQL it didn't build. Lint the target
/// model, plus any other soft-delete model whose table the raw text mentions (the
/// joined-table case).
fn check_raw_soft_delete_gap(raw: &RawSql, ti: usize, cx: &Cx, sink: &mut Sink) {
    let text: String = raw
        .parts
        .iter()
        .filter_map(|p| match p {
            RawPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    for (mi, m) in cx.models.iter().enumerate() {
        let Some(sd) = &m.soft_delete else { continue };
        if mi == ti || mentions_table(&text, &m.table) {
            sink.warn(
                code::RAW_SOFT_DELETE_GAP,
                raw.span,
                format!(
                    "raw SQL on soft-delete model `{}`: engine can't verify the `{}` tombstone filter — confirm it",
                    m.name, sd.field
                ),
            );
        }
    }
}

/// Whether raw SQL text contains `table` as a standalone word (identifier-boundary
/// match, so `user` never fires on `user_event`).
fn mentions_table(text: &str, table: &str) -> bool {
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(i) = text[from..].find(table) {
        let start = from + i;
        let end = start + table.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Resolve a member by name to its `Mapped` type, reporting an unknown-field error
/// (and returning `None`) when it doesn't exist.
fn mapped_member<'a>(
    m: &'a RModel,
    name: &str,
    cx: &Cx,
    ti: usize,
    at: &Ident,
    sink: &mut Sink,
) -> Option<resolve::Mapped<'a>> {
    match m.member(name).map(|mm| &mm.kind) {
        Some(MemberKind::Scalar { ty, .. }) => Some(resolve::Mapped::Scalar(*ty)),
        Some(k @ (MemberKind::Forward { .. } | MemberKind::Inverse { .. })) => {
            Some(resolve::Mapped::Relation(k.target().unwrap()))
        }
        None => {
            unknown_field(cx, ti, at, sink);
            None
        }
    }
}

/// Validate `where`/`order`/`page` clauses; returns whether an `order` is present.
fn check_clauses(
    clauses: &[Clause],
    mi: usize,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) -> bool {
    let mut has_order = false;
    for c in clauses {
        match c {
            Clause::Where(p) => resolve::check_predicate(p, Some(mi), cx, params, sink),
            Clause::Order(terms) => {
                has_order = true;
                for t in terms {
                    resolve::check_sort_term(t, mi, cx, sink);
                }
            }
            Clause::Page(_) => {}
            // Validated by the index pass (indexes.rs): satisfies W0103, or is
            // itself flagged stale (W0105) when the query turns out indexed.
            Clause::Unindexed(_) => {}
            // Aggregation clauses: legal only on an aggregate query, where
            // `check_agg_query` validates them; `reject_agg_clauses` reports them here.
            Clause::GroupBy(_) | Clause::Having(_) => {}
        }
    }
    has_order
}

/// `group by` / `having` on a non-aggregate query is `E0243`.
fn reject_agg_clauses(q: &Query, clauses: &[Clause], sink: &mut Sink) {
    for c in clauses {
        match c {
            Clause::GroupBy(cols) => {
                let span = cols
                    .first()
                    .and_then(|p| p.segments.first())
                    .map_or(q.span, |s| s.span);
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
/// normally; every non-aggregate projected column must be a `group by` column (`E0242`);
/// `order`/`having` name projected columns (`E0242`); `page` is rejected (`E0244`).
fn check_agg_query(
    q: &Query,
    clauses: &[Clause],
    mi: usize,
    body: &[ShapeField],
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let (out_names, group_cols) = summarize_agg_shape(body);
    let (group_paths, having, _present) = agg_clauses(clauses);

    // Group-by columns must resolve against the model.
    for p in &group_paths {
        if let Some(term) = resolve::resolve_path(p, mi, cx, sink) {
            resolve::reject_opaque(&term, p, "group", sink);
        }
    }
    // Group-by consistency: every projected non-aggregate column must be grouped.
    for gc in &group_cols {
        if !group_paths.iter().any(|gp| same_path(gp, &gc.path)) {
            let span = gc.path.segments.last().map_or(q.span, |s| s.span);
            sink.error_note(
                code::AGG_GROUP_BY,
                span,
                format!(
                    "projected column `{}` must be a `group by` column",
                    join_path(&gc.path)
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
            Clause::GroupBy(_) | Clause::Having(_) | Clause::Unindexed(_) => {}
        }
    }

    if let Some(hp) = having {
        check_having(hp, &out_names, params, q.span, sink);
    }
}

/// Every left operand in a `having` predicate must name a projected column of the
/// aggregate shape (an aggregate alias or a group column), so it maps to something the
/// grouped result actually has (`E0242`).
fn check_having(
    p: &Predicate,
    out_names: &[String],
    params: &[String],
    qspan: Span,
    sink: &mut Sink,
) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            check_having(a, out_names, params, qspan, sink);
            check_having(b, out_names, params, qspan, sink);
        }
        Predicate::Not(inner) => check_having(inner, out_names, params, qspan, sink),
        Predicate::Cmp { path, value, .. } => {
            check_having_path(path, out_names, qspan, sink);
            if let Value::Param(pr) = value {
                resolve::check_param_ref(pr, params, sink);
            }
        }
        Predicate::InList { path, values } => {
            check_having_path(path, out_names, qspan, sink);
            for v in values {
                if let Value::Param(pr) = v {
                    resolve::check_param_ref(pr, params, sink);
                }
            }
        }
        Predicate::Bare(path) => check_having_path(path, out_names, qspan, sink),
        // A raw predicate term / named-filter call is a leaf escape — left unchecked.
        Predicate::FilterCall { .. } | Predicate::Raw(_) => {}
    }
}

fn check_having_path(path: &Path, out_names: &[String], qspan: Span, sink: &mut Sink) {
    let ok = path.segments.len() == 1 && out_names.contains(&path.segments[0].node);
    if !ok {
        let span = path.segments.last().map_or(qspan, |s| s.span);
        sink.error_note(
            code::AGG_GROUP_BY,
            span,
            format!(
                "`having` references `{}`, which the aggregate shape doesn't project",
                join_path(path)
            ),
            "filter on a projected aggregate alias or group column",
        );
    }
}

fn join_path(p: &Path) -> String {
    p.segments
        .iter()
        .map(|s| s.node.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// A `get` is validly keyed if some equality-constrained column is unique.
fn get_is_keyed(q: &Query, ti: usize, cx: &Cx) -> bool {
    let m = cx.model(ti);
    match &q.body {
        QueryBody::Block(s) => {
            let smi = cx.find(&s.model.node).unwrap_or(ti);
            let sm = cx.model(smi);
            let mut cols = Vec::new();
            for c in &s.clauses {
                if let Clause::Where(p) = c {
                    collect_eq_cols(p, &mut cols);
                }
            }
            cols.iter().any(|c| sm.is_unique(c))
        }
        _ => q.params.iter().any(|p| match &p.binding {
            None => m.is_unique(&p.name.node),
            Some(ParamBinding::ColOp { op: Op::Eq, col }) => m.is_unique(&col.node),
            _ => false,
        }),
    }
}

/// Collect single-segment columns constrained by equality anywhere in a predicate.
fn collect_eq_cols(p: &Predicate, out: &mut Vec<String>) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            collect_eq_cols(a, out);
            collect_eq_cols(b, out);
        }
        Predicate::Not(inner) => collect_eq_cols(inner, out),
        Predicate::Cmp {
            path, op: Op::Eq, ..
        } if path.segments.len() == 1 => {
            out.push(path.segments[0].node.clone());
        }
        _ => {}
    }
}

// ---------- mutations ------------------------------------------------------

pub fn check_mutation(m: &Mutation, cx: &Cx, sink: &mut Sink) -> Option<RMutation> {
    // A mutation returns its written row once — never a stream (E0202).
    if m.ret.stream {
        sink.error_note(
            code::STREAM_MUTATION,
            m.ret.ty.span,
            format!("mutation `{}` cannot return a stream", m.name.node),
            "a write returns its row once; declare a stream query for the read",
        );
    }
    let params: Vec<String> = m.params.iter().map(|p| p.name.node.clone()).collect();
    for p in &m.params {
        if let Some(d) = &p.default {
            resolve::check_default(d, sink);
        }
    }
    let ret = if m.ret.ack {
        resolve_ack_return(m, cx, sink)?
    } else {
        let ret = resolve_return(&m.ret, None, cx, sink)?;
        check_shape_on_real_delete(m, &ret, cx, sink);
        // A mutation reads back a written row, not a group — an aggregate shape is not a
        // mutation return (E0245).
        if ret
            .shape
            .as_deref()
            .and_then(|n| cx.shape_bodies.get(n).copied())
            .is_some_and(is_agg_shape)
        {
            sink.error_note(
                code::AGG_COMPOSE,
                m.ret.ty.span,
                format!("mutation `{}` returns an aggregate shape", m.name.node),
                "a write reads back one written row — return a per-row shape",
            );
        }
        ret
    };
    // At the top level there is no enclosing `tx`, so no step binding is in scope.
    // A mutation may opt out of `@scope` on its write models  — that both drops
    // the injected guard and lets a `create` assign the (otherwise engine-managed)
    // scope column, so the flag rides into every write check.
    let unscoped = m.unscoped.is_some();
    let bindings = Bindings::default();
    for stmt in &m.body {
        check_write(
            stmt,
            cx,
            &params,
            &bindings,
            m.scoped.as_ref(),
            unscoped,
            sink,
        );
    }

    check_keyless_readback(m, &ret, cx, sink);

    // Scope acknowledgement: a mutation touching a scoped
    // model must name it (`scoped …`) or opt out (`unscoped(…)`) — E0182/E0183/E0185.
    let touched = crate::scope::touched_mutation(m, ret.shape.as_deref(), &ret.model, cx);
    crate::scope::check_ack(m.scoped.as_ref(), unscoped, &touched, cx, m.span, sink);
    // E0186: each `create` on a scoped model must name a full `@scope` alternative so the
    // engine can auto-set its columns from `$ctx` (no unowned row). Skipped for `unscoped`.
    crate::scope::check_create_sat(m, cx, sink);
    // `unscoped` on a mutation touching no scope is stale (W0106).
    if let Some(u) = &m.unscoped {
        if touched.is_empty() {
            sink.warn_note(
                code::STALE_UNSCOPED,
                u.span,
                format!(
                    "`unscoped` on mutation `{}` has no scope to opt out of",
                    m.name.node
                ),
                "drop it, or add `@scope Name` to a written model",
            );
        }
    }
    // Shard key : the return model's `@scope` owner field — a `tx` is a single-shard
    // unit , so the whole mutation routes on the primary written model's owner. An
    // `unscoped` mutation  disables scope and so has no owning shard.
    let shard_key = if unscoped {
        None
    } else {
        cx.find(&ret.model)
            .and_then(|mi| cx.model(mi).shard_key_ctx_field())
    };
    let scope_inject = crate::scope::resolve_inject(m.scoped.as_ref(), unscoped, &touched, cx);
    // The `$ctx` requirement derives from the same chosen alternative(s) as the
    // injection, so the bag always carries exactly the injected `:ctx_<field>`s.
    let scope_reqs = crate::scope::inject_ctx_reqs(m.scoped.as_ref(), unscoped, &touched, cx);
    Some(RMutation {
        name: m.name.node.clone(),
        span: m.span,
        guard: m.guard.as_ref().map(|g| g.node.clone()),
        ctx_requires: crate::ctx::collect_mutation(m, cx, scope_reqs),
        ret_model: ret.model,
        ack: m.ret.ack,
        ret_shape: ret.shape,
        shard_key,
        scope_inject,
    })
}

/// A create on a keyless return model has no generated `id` to read the row back by, so a
/// declared-shape return must key on a `(unique)` column the create sets. If it sets none,
/// reject rather than emit a re-select the runtime can't key (E0264). An `-> ok` mutation
/// reads nothing back, so it is exempt.
fn check_keyless_readback(m: &Mutation, ret: &Resolved, cx: &Cx, sink: &mut Sink) {
    if m.ret.ack {
        return;
    }
    let Some(rmodel) = cx.find(&ret.model).map(|i| cx.model(i)) else {
        return;
    };
    if rmodel.no_id
        && creates_model(&m.body, &ret.model)
        && !create_sets_unique(&m.body, &ret.model, rmodel)
    {
        sink.error_note(
            code::KEYLESS_CREATE,
            m.span,
            format!(
                "mutation `{}` creates keyless `{}` but sets no unique column to read it back by",
                m.name.node, ret.model
            ),
            "a `@no_id` model has no generated `id` — assign a `(unique)` column in the `create`, or return `-> ok`",
        );
    }
}

/// A write's effect on its target row, for the `-> ok` / declared-shape rules.
enum WriteEffect<'a> {
    /// A real DELETE — plain-model `delete` or `hard delete`: the row is removed.
    RealDelete(&'a Ident),
    /// create / update / restore / soft `delete` (tombstone): a row survives to
    /// read back. Carries the verb for the diagnostic.
    Surviving(&'a Ident, &'static str),
    /// A raw write — its effect is outside the engine's knowledge.
    Raw,
}

/// Classify each write of the body ( `tx` blocks flattened, execution order).
fn write_effects<'a>(body: &'a [WriteStmt], cx: &Cx) -> Vec<WriteEffect<'a>> {
    let mut out = Vec::new();
    for stmt in body {
        match stmt {
            WriteStmt::Create { model, .. } => out.push(WriteEffect::Surviving(model, "create")),
            WriteStmt::Update { model, .. } => out.push(WriteEffect::Surviving(model, "update")),
            WriteStmt::Restore { model, .. } => out.push(WriteEffect::Surviving(model, "restore")),
            WriteStmt::HardDelete { model, .. } => out.push(WriteEffect::RealDelete(model)),
            WriteStmt::Delete { model, .. } => {
                let soft = cx
                    .find(&model.node)
                    .is_some_and(|mi| cx.model(mi).soft_delete.is_some());
                if soft {
                    out.push(WriteEffect::Surviving(model, "delete (soft)"));
                } else {
                    out.push(WriteEffect::RealDelete(model));
                }
            }
            WriteStmt::Tx(inner) => out.extend(write_effects(inner, cx)),
            WriteStmt::Raw(_) => out.push(WriteEffect::Raw),
        }
    }
    out
}

/// Resolve an `-> ok` mutation: every write must be a real DELETE (a raw write is
/// allowed — its effect is the author's), and the primary model — the one scope,
/// sharding, and the 404-on-zero-rows check ride on — is the first real DELETE's.
fn resolve_ack_return(m: &Mutation, cx: &Cx, sink: &mut Sink) -> Option<Resolved> {
    let mut primary: Option<&Ident> = None;
    for e in write_effects(&m.body, cx) {
        match e {
            WriteEffect::RealDelete(model) => primary = primary.or(Some(model)),
            WriteEffect::Surviving(model, verb) => {
                sink.error_note(
                    code::ACK_SURVIVING,
                    m.ret.ty.span,
                    format!(
                        "mutation `{}` returns `ok` but `{verb} {}` leaves a surviving row",
                        m.name.node, model.node
                    ),
                    "a surviving write reads its row back — declare its shape; `-> ok` is for real DELETEs",
                );
                return None;
            }
            WriteEffect::Raw => {}
        }
    }
    let Some(model) = primary else {
        sink.error_note(
            code::ACK_SURVIVING,
            m.ret.ty.span,
            format!(
                "mutation `{}` returns `ok` but performs no real DELETE",
                m.name.node
            ),
            "`-> ok` acknowledges a destructive write: a plain-model `delete` or `hard delete`",
        );
        return None;
    };
    // An unknown model is reported by the write check at its own site.
    cx.find(&model.node)?;
    Some(Resolved {
        model: model.node.clone(),
        shape: None,
    })
}

/// A declared shape needs a surviving row. When every write on the return model is
/// a real DELETE, the re-select has nothing to read — the response could never
/// decode as the shape (E0220).
fn check_shape_on_real_delete(m: &Mutation, ret: &Resolved, cx: &Cx, sink: &mut Sink) {
    let mut deletes_ret = false;
    let mut survives_ret = false;
    for e in write_effects(&m.body, cx) {
        match e {
            WriteEffect::RealDelete(model) if model.node == ret.model => deletes_ret = true,
            WriteEffect::Surviving(model, _) if model.node == ret.model => survives_ret = true,
            _ => {}
        }
    }
    if deletes_ret && !survives_ret {
        sink.error_note(
            code::SHAPE_ON_DELETE,
            m.ret.ty.span,
            format!(
                "mutation `{}` performs a real DELETE of `{}` — no row survives to read back as `{}`",
                m.name.node, ret.model, m.ret.ty.node
            ),
            "a destructive mutation acknowledges instead of reading back: declare `-> ok`",
        );
    }
}

/// The query's `page` clause, if any (inline or block body).
fn page_clause(body: &QueryBody) -> Option<&PageClause> {
    let clauses: &[Clause] = match body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        _ => return None,
    };
    clauses.iter().find_map(|c| match c {
        Clause::Page(p) => Some(p),
        _ => None,
    })
}

/// The query's effective sort terms: its `order` clause, else the model's `@sort`.
fn effective_sort<'a>(body: &'a QueryBody, model: &'a RModel) -> &'a [SortTerm] {
    let clauses: &[Clause] = match body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        _ => return &model.sort,
    };
    clauses
        .iter()
        .find_map(|c| match c {
            Clause::Order(t) => Some(t.as_slice()),
            _ => None,
        })
        .unwrap_or(&model.sort)
}

/// Whether the mutation body creates a row of `model` (recursing into `tx`).
fn creates_model(body: &[WriteStmt], model: &str) -> bool {
    body.iter().any(|w| match w {
        WriteStmt::Create { model: m, .. } => m.node == model,
        WriteStmt::Tx(inner) => creates_model(inner, model),
        _ => false,
    })
}

/// Whether every `create` of `model` in the body assigns a `(unique)` column — the
/// read-back key a keyless model needs (no generated `id`). Fires per create so a
/// mixed batch is only clean when each keyless create is keyable.
fn create_sets_unique(body: &[WriteStmt], model: &str, rmodel: &RModel) -> bool {
    body.iter().all(|w| match w {
        WriteStmt::Create {
            model: m, assigns, ..
        } if m.node == model => assigns.iter().any(|a| rmodel.is_unique(&a.col.node)),
        WriteStmt::Tx(inner) => create_sets_unique(inner, model, rmodel),
        _ => true,
    })
}

fn check_write(
    stmt: &WriteStmt,
    cx: &Cx,
    params: &[String],
    bindings: &Bindings,
    scoped: Option<&Scoped>,
    unscoped: bool,
    sink: &mut Sink,
) {
    match stmt {
        WriteStmt::Create {
            model,
            assigns,
            conflict,
            binding: _,
        } => {
            if let Some(mi) = write_model(model, cx, sink) {
                for a in assigns {
                    check_assign(
                        a, mi, cx, params, bindings, /* in_update = */ false, sink,
                    );
                }
                check_scope_assign(mi, assigns, unscoped, cx, sink);
                check_create_required(mi, assigns, model, scoped, unscoped, cx, sink);
                if let Some(oc) = conflict {
                    check_upsert(oc, mi, assigns, scoped, unscoped, cx, params, sink);
                }
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            if let Some(mi) = write_model(model, cx, sink) {
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
                for a in assigns {
                    check_assign(
                        a, mi, cx, params, bindings, /* in_update = */ true, sink,
                    );
                }
            }
        }
        WriteStmt::Delete { model, where_ } | WriteStmt::HardDelete { model, where_ } => {
            if let Some(mi) = write_model(model, cx, sink) {
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
            }
        }
        WriteStmt::Restore { model, where_ } => {
            if let Some(mi) = write_model(model, cx, sink) {
                if cx.model(mi).soft_delete.is_none() {
                    sink.error(
                        code::RESTORE_NOT_SOFT,
                        model.span,
                        format!(
                            "`restore` requires a @soft_delete model; `{}` has none",
                            model.node
                        ),
                    );
                }
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
            }
        }
        WriteStmt::Tx(inner) => check_tx(inner, cx, params, bindings, scoped, unscoped, sink),
        WriteStmt::Raw(raw) => {
            for part in &raw.parts {
                if let RawPart::Param(pr) = part {
                    resolve::check_param_ref(pr, params, sink);
                }
            }
        }
    }
}

/// Named step bindings (`create … as name`): a binding reaches any *prior* step. Pre-scan
/// every binding name in this tx so a forward reference is distinguishable from a plain
/// unbound name (both E0281); then descend, growing the reachable set after each bound
/// create and flagging a binding that shadows a param or duplicates another (E0280).
fn check_tx(
    inner: &[WriteStmt],
    cx: &Cx,
    params: &[String],
    bindings: &Bindings,
    scoped: Option<&Scoped>,
    unscoped: bool,
    sink: &mut Sink,
) {
    let mut binds = bindings.clone();
    for s in inner {
        if let WriteStmt::Create {
            binding: Some(b), ..
        } = s
        {
            binds.all.insert(b.node.clone());
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for s in inner {
        check_write(s, cx, params, &binds, scoped, unscoped, sink);
        let WriteStmt::Create {
            model,
            binding: Some(b),
            ..
        } = s
        else {
            continue;
        };
        if params.iter().any(|p| p == &b.node) {
            sink.error_note(
                code::BINDING_SHADOW,
                b.span,
                format!("step binding `{}` shadows a parameter", b.node),
                "rename the binding — `$…` must name one thing",
            );
        } else if !seen.insert(b.node.as_str()) {
            sink.error(
                code::BINDING_SHADOW,
                b.span,
                format!("duplicate step binding `{}` in this `tx`", b.node),
            );
        }
        if let Some(mi) = write_model(model, cx, &mut Sink::default()) {
            binds.resolved.insert(b.node.clone(), mi);
        }
    }
}

fn check_assign(
    a: &Assign,
    mi: usize,
    cx: &Cx,
    params: &[String],
    bindings: &Bindings,
    in_update: bool,
    sink: &mut Sink,
) {
    let Some(member) = cx.model(mi).member(&a.col.node) else {
        unknown_field(cx, mi, &a.col, sink);
        return;
    };
    // An opaque column holds a value the engine cannot construct or validate, so it is
    // excluded from every write (`E0273`); the DB or a raw migration owns it.
    if let Some(spec) = member.kind.opaque() {
        sink.error_note(
            code::OPAQUE_ASSIGN,
            a.col.span,
            format!(
                "cannot write `{}` — a {} column is opaque",
                a.col.node,
                spec.render()
            ),
            "the engine does not model this type; set it from a raw migration or a DB default",
        );
        return;
    }
    // An arithmetic RHS (`total = total + $n`) is its own world: numeric-only, and
    // valid only in an `update` (a `create` has no existing row to reference).
    let Some(value) = a.value.as_value() else {
        resolve::check_assign_arith(
            &a.value,
            &member.kind,
            &a.col,
            mi,
            in_update,
            cx,
            params,
            sink,
        );
        return;
    };
    // A `$name.field` reference to a prior `tx` step binding (D107): `$` unifies params
    // + step bindings, so a `$name` that is neither `$ctx` nor a declared param must be a
    // step binding — resolve it against the bound step's model, not the model being
    // assigned. Type-check happens inside; nothing else applies to a binding reference.
    if let Value::Param(pr) = value {
        if pr.name.node != "ctx" && !params.iter().any(|p| p == &pr.name.node) {
            check_binding_ref(pr, bindings, &member.kind, &a.col, cx, sink);
            return;
        }
    }
    // Assigning an enum column takes a bare variant (`status = paid`), not a column
    // path — check membership (E0154) instead of resolving it as a field.
    if let MemberKind::Scalar {
        enum_name: Some(en_name),
        ..
    } = &member.kind
    {
        if let Some(en) = cx.enum_(en_name) {
            if resolve::check_enum_operand(value, en, params, sink) {
                return;
            }
        }
    }
    resolve::check_value(value, Some(mi), cx, params, sink);
    // The assigned value's type must agree with the target column (E0153); silent
    // when either side failed to resolve above.
    resolve::check_assign_type(&member.kind, &a.col, value, mi, cx, sink);
}

/// On a scoped model  the `@scope` column is engine-managed on `create`: it is
/// auto-set from `$ctx.<field>` so a caller cannot plant a row outside their own scope
/// (cross-scope create is inexpressible). Assigning it is therefore an `E0181`, exactly
/// as assigning `id`/`@created` would be redundant/wrong — *unless* the mutation is
/// `unscoped` , in which case scope isn't injected at all and the caller owns the
/// column. Reports every offending assign.
fn check_scope_assign(mi: usize, assigns: &[Assign], unscoped: bool, cx: &Cx, sink: &mut Sink) {
    if unscoped {
        return;
    }
    // Every alternative's columns are engine-domain, not just the chosen one:
    // planting *any* scope column plants the row into an arbitrary scope.
    let scope_cols = crate::scope::all_scope_cols(mi, cx);
    for a in assigns {
        if scope_cols.iter().any(|f| f == &a.col.node) {
            sink.error_note(
                code::SCOPE_ASSIGN,
                a.col.span,
                format!(
                    "`{}` is `@scope`-managed on `create`; the engine sets it from `$ctx`",
                    a.col.node
                ),
                "a scoped create can't target another scope — drop the assign, or mark the mutation `unscoped(\"…\")`",
            );
        }
    }
}

/// A `create` must assign every *required* column: a non-optional, non-defaulted
/// stored column or forward FK. Engine-managed fields — the `id`, `@created` /
/// `@updated` timestamps, the `@soft_delete` field, and the `@scope` columns the
/// mutation's *chosen* alternative auto-sets from `$ctx` on insert — are set by
/// the engine, so they are exempt. A scope column outside the chosen alternative
/// (or any scope column on an `unscoped` create) is nobody's: the engine won't
/// set it and E0181 forbids assigning it on a scoped create, so it stays required
/// and shows here. Inverse edges own no column, so they never count. A missing
/// field is `E0146` (all missing fields reported in one error).
fn check_create_required(
    mi: usize,
    assigns: &[Assign],
    at: &Ident,
    scoped: Option<&Scoped>,
    unscoped: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    let m = cx.model(mi);
    let assigned: Vec<&str> = assigns.iter().map(|a| a.col.node.as_str()).collect();
    let scope_cols: Vec<(String, String)> =
        crate::scope::resolve_inject(scoped, unscoped, &[mi], cx)
            .into_iter()
            .flat_map(|si| si.terms)
            .collect();
    let managed = |name: &str| {
        name == "id"
            || m.created.as_deref() == Some(name)
            || m.updated.as_deref() == Some(name)
            || m.soft_delete.as_ref().map(|s| s.field.as_str()) == Some(name)
            || scope_cols.iter().any(|(f, _)| f == name)
    };
    // A required *opaque* column can never be supplied (E0273 forbids writing one), so a
    // create on this model is unwritable until the column is made nullable or defaulted.
    let unsuppliable: Vec<&str> = m
        .members
        .iter()
        .filter(|mem| is_required(&mem.kind) && mem.kind.opaque().is_some())
        .map(|mem| mem.name.as_str())
        .filter(|name| !managed(name))
        .collect();
    if !unsuppliable.is_empty() {
        sink.error_note(
            code::OPAQUE_ASSIGN,
            at.span,
            format!(
                "`create {}` cannot supply the opaque column{} {}",
                m.name,
                if unsuppliable.len() == 1 { "" } else { "s" },
                unsuppliable.join(", ")
            ),
            "an opaque `raw(…)` column is excluded from writes — make it nullable (`?`) or give it a `(default …)`",
        );
    }
    let missing: Vec<&str> = m
        .members
        .iter()
        .filter(|mem| is_required(&mem.kind) && mem.kind.opaque().is_none())
        .map(|mem| mem.name.as_str())
        .filter(|name| !managed(name) && !assigned.contains(name))
        .collect();
    if !missing.is_empty() {
        sink.error(
            code::CREATE_MISSING,
            at.span,
            format!(
                "`create {}` is missing required field{}: {}",
                m.name,
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ),
        );
    }
}

/// Validate an upsert's `on conflict (target) update { … }` (mutations.md): the target
/// must be a declared unique key each of whose columns the create sets, the `update`
/// branch is an ordinary update that may not move the key, and (safety) a soft-delete
/// model is out and a scoped model's target must carry its scope column(s).
#[allow(clippy::too_many_arguments)]
fn check_upsert(
    oc: &OnConflict,
    mi: usize,
    create_assigns: &[Assign],
    scoped: Option<&Scoped>,
    unscoped: bool,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let target: Vec<&str> = oc.target.iter().map(|t| t.node.as_str()).collect();
    check_conflict_target(oc, &target, mi, create_assigns, scoped, unscoped, cx, sink);

    // The update branch is an ordinary update — check its assigns — but it may not assign
    // a conflict column (moving the key would break the conflict + the read-back).
    let no_bindings = Bindings::default();
    for a in &oc.update {
        check_assign(
            a,
            mi,
            cx,
            params,
            &no_bindings,
            /* in_update = */ true,
            sink,
        );
        if target.iter().any(|t| *t == a.col.node) {
            sink.error_note(
                code::UPSERT_TARGET_SET,
                a.col.span,
                format!(
                    "the `on conflict update` branch assigns the conflict column `{}`",
                    a.col.node
                ),
                "the update runs on a conflict *of* this key — don't move it",
            );
        }
    }
}

/// The conflict target must name a key the database actually enforces, every column of it
/// must get a value on the create, and — on a scoped model — it must include the scope
/// columns, else a conflict could match (and the update modify) another scope's row.
#[allow(clippy::too_many_arguments)]
fn check_conflict_target(
    oc: &OnConflict,
    target: &[&str],
    mi: usize,
    create_assigns: &[Assign],
    scoped: Option<&Scoped>,
    unscoped: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    let m = cx.model(mi);

    // A tombstoned row still occupies its unique key, so a conflict would silently
    // update the tombstone instead of inserting — surprising and unsafe.
    if m.soft_delete.is_some() {
        sink.error_note(
            code::UPSERT_SOFT_DELETE,
            oc.span,
            format!(
                "`on conflict` is not allowed on the @soft_delete model `{}`",
                m.name
            ),
            "a tombstoned row still holds its unique key — an upsert would update it, not insert",
        );
    }

    if !is_unique_key(m, target) {
        sink.error_note(
            code::UPSERT_TARGET,
            oc.span,
            format!(
                "conflict target ({}) is not a unique key of `{}`",
                target.join(", "),
                m.name
            ),
            "name a `(unique)` column, a `@index (…) unique`, or the pk — a conflict needs a key the database enforces",
        );
    }

    // The scope columns this mutation auto-manages on the create (the chosen alternative).
    let scope_cols: Vec<String> = crate::scope::resolve_inject(scoped, unscoped, &[mi], cx)
        .into_iter()
        .flat_map(|si| si.terms)
        .map(|(field, _)| field)
        .collect();

    let assigned: Vec<&str> = create_assigns.iter().map(|a| a.col.node.as_str()).collect();
    for t in &oc.target {
        let set = assigned.contains(&t.node.as_str()) || scope_cols.iter().any(|c| c == &t.node);
        if !set {
            sink.error_note(
                code::UPSERT_TARGET_UNSET,
                t.span,
                format!("conflict column `{}` is not set by the create", t.node),
                "assign it in the create block (or let a `@scope` column supply it) so the conflict has a value",
            );
        }
    }

    for sc in &scope_cols {
        if !target.iter().any(|t| t == sc) {
            sink.error_note(
                code::UPSERT_SCOPE,
                oc.span,
                format!(
                    "conflict target on scoped `{}` omits the scope column `{sc}`",
                    m.name
                ),
                "add it to the conflict target so a conflict can only match a row in the caller's own scope",
            );
        }
    }
}

/// Whether `target` (field names) is a declared unique key of `m`: a single `(unique)`
/// column (or the pk `id`, always unique), or a `@index (…) unique` whose columns are
/// exactly the set (order-insensitive).
fn is_unique_key(m: &RModel, target: &[&str]) -> bool {
    if target.len() == 1 && m.is_unique(target[0]) {
        return true;
    }
    m.indexes.iter().any(|ix| {
        ix.unique
            && ix.columns.len() == target.len()
            && ix.columns.iter().all(|c| target.contains(&c.as_str()))
    })
}

/// A column the caller must supply on `create`: a non-optional scalar with no
/// default, or a non-optional forward FK (a custom-join edge has no FK column to
/// set, so it is excluded).
fn is_required(kind: &MemberKind) -> bool {
    match kind {
        MemberKind::Scalar {
            optional, default, ..
        } => !*optional && default.is_none(),
        MemberKind::Forward {
            optional,
            custom_join,
            ..
        } => !*optional && !*custom_join,
        MemberKind::Inverse { .. } => false,
    }
}

/// The `tx` step bindings in scope while checking a write (D107). `resolved` maps a
/// binding name reachable *now* (a create at a prior step, `create … as name`) to its
/// model; `all` is every binding name declared anywhere in the enclosing `tx`, so a
/// forward reference (`$x` used before its `as x`) reads distinctly from a plain typo.
#[derive(Clone, Default)]
struct Bindings {
    resolved: std::collections::HashMap<String, usize>,
    all: std::collections::HashSet<String>,
}

/// Resolve a `$name.field` reference to a `tx` step binding: `name` must name a binding
/// reachable from here (a prior step), the reference is field-access only (one segment),
/// and `field` must be a column of the bound step's model, its family agreeing with the
/// assigned column. `E0281` covers an unbound / forward-referenced name and a malformed
/// (non single-field) reference.
fn check_binding_ref(
    pr: &ParamRef,
    bindings: &Bindings,
    target: &MemberKind,
    col: &Ident,
    cx: &Cx,
    sink: &mut Sink,
) {
    let name = &pr.name.node;
    let Some(&mi) = bindings.resolved.get(name) else {
        let msg = if bindings.all.contains(name) {
            format!("`${name}` is bound by a later step — a step binding reaches only prior steps")
        } else {
            format!("`${name}` is not a parameter or a bound step (`create … as {name}`)")
        };
        sink.error(code::BINDING_UNBOUND, pr.name.span, msg);
        return;
    };
    let [field] = pr.path.as_slice() else {
        sink.error(
            code::BINDING_UNBOUND,
            pr.name.span,
            format!("reference a bound step's field as `${name}.field` (e.g. `${name}.id`)"),
        );
        return;
    };
    let Some(member) = cx.model(mi).member(&field.node) else {
        unknown_field(cx, mi, field, sink);
        return;
    };
    resolve::check_field_assign_type(target, col, &member.kind, sink);
}

fn write_model(name: &Ident, cx: &Cx, sink: &mut Sink) -> Option<usize> {
    if let Some(i) = cx.find(&name.node) {
        Some(i)
    } else {
        sink.error(
            code::UNKNOWN_MODEL,
            name.span,
            format!("unknown model `{}`", name.node),
        );
        None
    }
}

// ---------- filters --------------------------------------------------------

pub fn check_filter(f: &NamedFilter, cx: &Cx, sink: &mut Sink) -> RFilter {
    let params: Vec<String> = f.params.iter().map(|p| p.name.node.clone()).collect();
    for p in &f.params {
        if let Some(d) = &p.default {
            resolve::check_default(d, sink);
        }
    }
    // A named filter has no caller model at declaration, so column paths are not
    // bound here (they resolve against whichever model calls it) — only params,
    // nested filter calls, and functions are checked.
    resolve::check_predicate(&f.pred, None, cx, &params, sink);
    RFilter {
        name: f.name.node.clone(),
        span: f.span,
        arity: f.params.len(),
    }
}

// ---------- shared ---------------------------------------------------------

fn unknown_field(cx: &Cx, mi: usize, id: &Ident, sink: &mut Sink) {
    sink.error(
        code::UNKNOWN_FIELD,
        id.span,
        format!("`{}` has no field `{}`", cx.model(mi).name, id.node),
    );
}
