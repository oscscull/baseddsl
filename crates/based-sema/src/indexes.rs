//! The index requirements + the index lints.
//!
//! Runs after every query/shape/mutation is resolved, because the whole point is
//! closed-world reasoning: the access layer *is* the set of generated SQL, so the
//! engine can know which indexes queries need and which declared indexes nothing
//! uses.
//!
//! Two index facts carry independent write/disk cost and are load-bearing, so they
//! are written in source, not silently derived:
//!   * **E0260 `unindexed`**: a query — or a mutation's `update`/`delete`/`restore`
//!     `where`, which scans the same way — whose root filter pattern no available
//!     index serves, and a relation join key some query/shape traverses that no
//!     `@index` covers. Both are errors, satisfied by an `@index` (the editor
//!     autofix inserts one) or by the visible `unindexed(max_rows: N)` /
//!     `unindexed(unsafe)` annotation (a query-body clause; a bulk write has no such
//!     clause, so it can't opt out — it must be indexed).
//!   * **W0104 `useless-index`**: a declared non-unique index nothing in the access
//!     layer (queries *and* mutation `where`s) filters, sorts, or joins on — pure
//!     write tax. (Unique indexes are constraints, not perf, so they are exempt.)
//!     W0105 flags the inverse staleness: an `unindexed` annotation on a query that
//!     turns out indexed.
//!
//! The pattern model is coarse on purpose (same spirit as operand typing): eq/
//! range columns off the conjunctive spine, first-column index matching. It aims
//! to catch "this query has no index at all", not to re-derive the planner. A
//! predicate with an `or` or a raw atom is opaque — the filter check stays silent
//! rather than guessing (precision over recall).

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;
use crate::resolve::Cx;

/// Run the index requirement checks + lints.
#[allow(clippy::too_many_arguments)]
pub fn run(
    models_ast: &[&Model],
    queries_ast: &[&Query],
    shapes_ast: &[&Shape],
    mutations_ast: &[&Mutation],
    rqueries: &[RQuery],
    rmutations: &[RMutation],
    cx: &Cx,
    sink: &mut Sink,
) {
    let ast_by_name: HashMap<&str, &Query> = queries_ast
        .iter()
        .map(|q| (q.name.node.as_str(), *q))
        .collect();
    let shape_by_name: HashMap<&str, &Shape> = shapes_ast
        .iter()
        .map(|s| (s.name.node.as_str(), *s))
        .collect();

    // ---- collect: one pattern per query, one usage pool per model ----
    let mut usage: Vec<Usage> = cx.models.iter().map(|_| Usage::default()).collect();
    let mut patterns: Vec<(usize, Pattern)> = Vec::new(); // (model idx, pattern)

    for rq in rqueries {
        let Some(mi) = cx.find(&rq.target) else {
            continue;
        };
        let Some(ast) = ast_by_name.get(rq.name.as_str()).copied() else {
            continue;
        };
        let mut pat = query_pattern(ast, rq, mi, cx, &mut usage);

        // A shape only creates traversal demand through a query that returns it — its
        // join reaches ride on that query's pattern (so the query's `unindexed(…)`
        // opt-out covers them too).
        if let Some(shape) = rq.ret_shape.as_deref().and_then(|s| shape_by_name.get(s)) {
            shape_demand(&shape.body, mi, cx, &mut usage, &mut pat.joins);
        }
        patterns.push((mi, pat));
    }

    // A mutation's `update`/`delete`/`restore` `where` scans exactly like a query's
    // — feed each into the same pool so E0260 flags an unindexed bulk write and
    // W0104 counts a column a mutation filters on as used.
    let rmut_by_name: HashMap<&str, &RMutation> =
        rmutations.iter().map(|m| (m.name.as_str(), m)).collect();
    for mu in mutations_ast {
        let inject = rmut_by_name
            .get(mu.name.node.as_str())
            .map_or(&[][..], |rm| rm.scope_inject.as_slice());
        for stmt in &mu.body {
            collect_write(stmt, &mu.name.node, inject, cx, &mut usage, &mut patterns);
        }
    }

    // ---- E0260 / W0105: pattern vs available indexes -----------------------
    for (mi, pat) in &patterns {
        check_pattern(pat, *mi, cx, sink);
    }

    // ---- W0104: declared indexes nothing uses --------------------------------
    for (mi, ast) in models_ast.iter().enumerate() {
        lint_useless(ast, mi, &usage[mi], cx, sink);
    }
}

// ---------- per-model usage pool -------------------------------------------

/// Everything the access layer touches on one model, pooled across queries.
/// Feeds the useless-index lint (broad: a column mentioned anywhere counts as
/// "used", so W0104 under-fires rather than over-fires).
#[derive(Default)]
struct Usage {
    /// Columns filtered (eq or range) or sorted on, by any query — including
    /// filters that land here through a relation reach from another model.
    cols: Vec<String>,
    /// Forward fields whose FK column a traversed inverse edge joins through
    /// (`via` of each traversed `Inverse`) — pooled so a declared join-key index
    /// counts as used.
    join_fields: Vec<String>,
}

impl Usage {
    fn touch(&mut self, col: &str) {
        if !self.cols.iter().any(|c| c == col) {
            self.cols.push(col.to_string());
        }
    }
    fn join(&mut self, via: &str) {
        if !self.join_fields.iter().any(|c| c == via) {
            self.join_fields.push(via.to_string());
        }
    }
}

// ---------- per-query pattern -----------------------------------------------

/// The root-table access pattern of one query: what an index must lead with to
/// keep it from scanning.
struct Pattern {
    name: String,
    span: Span,
    /// Local fields under an equality-shaped constraint (`=`, `in`, bare bool).
    eq: Vec<String>,
    /// Local fields under a range-shaped constraint (`< > <= >= ~`).
    range: Vec<String>,
    /// Leading sort field when local (query `order` first, else model `@sort`).
    sort: Option<String>,
    /// Join keys this query traverses: `(model idx, forward `via` field)`. Each is
    /// an index the join runs through — the FK on the far model. Deduped.
    joins: Vec<(usize, String)>,
    paginated: bool,
    /// `or` / raw atom seen: the filter is beyond first-column reasoning, so the
    /// filter check stays silent instead of guessing (joins still check).
    opaque: bool,
    annotation: Option<Unindexed>,
}

impl Pattern {
    fn new(name: String, span: Span, paginated: bool) -> Self {
        Self {
            name,
            span,
            eq: Vec::new(),
            range: Vec::new(),
            sort: None,
            joins: Vec::new(),
            paginated,
            opaque: false,
            annotation: None,
        }
    }
}

fn query_pattern(q: &Query, rq: &RQuery, mi: usize, cx: &Cx, usage: &mut [Usage]) -> Pattern {
    let mut pat = Pattern::new(rq.name.clone(), rq.span, rq.paginated);

    // A raw body is opaque SQL: no filter pattern to reason about, so the
    // missing-index check stays silent (same treatment as a raw predicate atom).
    if matches!(q.body, QueryBody::Raw(_)) {
        pat.opaque = true;
        return pat;
    }

    // Params are filters on bare/inline queries (same-name / `-> edge` / `op col`).
    // On a block query they only enter through `$refs` inside the predicate.
    if !matches!(q.body, QueryBody::Block(_)) {
        for p in &q.params {
            match &p.binding {
                None => pat.add(mi, Op::Eq, &p.name.node, usage),
                Some(ParamBinding::Edge(e)) => pat.add(mi, Op::Eq, &e.node, usage),
                Some(ParamBinding::ColOp { op, col }) => pat.add(mi, *op, &col.node, usage),
            }
        }
    }

    let clauses: &[Clause] = match &q.body {
        QueryBody::Bare | QueryBody::Raw(_) => &[],
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
    };
    let mut has_order = false;
    for c in clauses {
        match c {
            Clause::Where(p) => pat.walk(p, mi, cx, usage, &mut Vec::new()),
            Clause::Order(terms) => {
                has_order = true;
                pat.note_sort(terms, mi, cx, usage);
            }
            Clause::Page(_) => {}
            Clause::Unindexed(u) => pat.annotation = Some(u.clone()),
            // Aggregation clauses don't drive the FK-join index inference (the WHERE row
            // filter still does, via `Clause::Where` above).
            Clause::GroupBy(_) | Clause::Having(_) => {}
        }
    }
    // Sort cascade: the model default applies when the query is bare.
    if !has_order && !cx.model(mi).sort.is_empty() {
        let terms = cx.model(mi).sort.clone();
        pat.note_sort(&terms, mi, cx, usage);
    }
    // The scope this query injects rides into it, filters included — the *chosen*
    // alternative's columns (an `unscoped` query injects none).
    for si in &rq.scope_inject {
        if si.model == cx.model(mi).name {
            for (col, _) in &si.terms {
                pat.add(mi, Op::Eq, col, usage);
            }
        }
    }
    pat
}

/// Turn a write statement's `where` into a root-table access pattern (recursing
/// through `tx`). `create` has no `where`; `raw` has no bound model. A mutation
/// carries no `unindexed(…)` clause (that is a query-body annotation), so a
/// scanning bulk write can't be annotated away — it must be indexed.
fn collect_write(
    stmt: &WriteStmt,
    mut_name: &str,
    inject: &[ScopeInject],
    cx: &Cx,
    usage: &mut [Usage],
    patterns: &mut Vec<(usize, Pattern)>,
) {
    let (model, where_) = match stmt {
        WriteStmt::Update { model, where_, .. }
        | WriteStmt::Delete { model, where_ }
        | WriteStmt::HardDelete { model, where_ }
        | WriteStmt::Restore { model, where_ } => (model, where_),
        WriteStmt::Tx(inner) => {
            for s in inner {
                collect_write(s, mut_name, inject, cx, usage, patterns);
            }
            return;
        }
        WriteStmt::Create { .. } | WriteStmt::Raw(_) => return,
    };
    let Some(mi) = cx.find(&model.node) else {
        return;
    };
    patterns.push((
        mi,
        write_pattern(mut_name, model.span, where_, mi, inject, cx, usage),
    ));
}

/// The access pattern of one write `where`: its conjunctive eq/range spine plus the
/// model's `@scope` (which rides into every write). No sort or pagination
/// applies to a write, and there is no annotation to suppress it.
fn write_pattern(
    name: &str,
    span: Span,
    where_: &Predicate,
    mi: usize,
    inject: &[ScopeInject],
    cx: &Cx,
    usage: &mut [Usage],
) -> Pattern {
    let mut pat = Pattern::new(name.to_string(), span, false);
    pat.walk(where_, mi, cx, usage, &mut Vec::new());
    for si in inject {
        if si.model == cx.model(mi).name {
            for (col, _) in &si.terms {
                pat.add(mi, Op::Eq, col, usage);
            }
        }
    }
    pat
}

impl Pattern {
    /// Record a constraint on a *local* field of the pattern's model.
    fn add(&mut self, mi: usize, op: Op, field: &str, usage: &mut [Usage]) {
        let bucket = match op {
            Op::Eq | Op::In => &mut self.eq,
            Op::Gt | Op::Lt | Op::Ge | Op::Le | Op::Like => &mut self.range,
            // `!=` / `has` exclude nothing an index can lead with.
            Op::Ne | Op::Has => return,
        };
        if !bucket.iter().any(|c| c == field) {
            bucket.push(field.to_string());
        }
        usage[mi].touch(field);
    }

    /// Walk a predicate's conjunctive spine collecting eq/range fields; relation
    /// reaches become join demand + remote usage. Named filters expand against
    /// the call-site model , guarded against self-reference.
    fn walk(
        &mut self,
        pred: &Predicate,
        mi: usize,
        cx: &Cx,
        usage: &mut [Usage],
        in_filters: &mut Vec<String>,
    ) {
        match pred {
            Predicate::And(a, b) => {
                self.walk(a, mi, cx, usage, in_filters);
                self.walk(b, mi, cx, usage, in_filters);
            }
            Predicate::Or(a, b) => {
                // Both branches still count as *usage* (an index either serves is
                // not useless), but the pattern is beyond first-column reasoning.
                self.opaque = true;
                self.walk(a, mi, cx, usage, in_filters);
                self.walk(b, mi, cx, usage, in_filters);
            }
            Predicate::Not(p) => self.walk(p, mi, cx, usage, in_filters),
            Predicate::Cmp { path, op, value } => {
                self.note_path(path, mi, *op, cx, usage);
                if let Value::Path(p) = value {
                    self.note_path(p, mi, Op::Eq, cx, usage);
                }
            }
            Predicate::InList { path, values } => {
                self.note_path(path, mi, Op::In, cx, usage);
                for v in values {
                    if let Value::Path(p) = v {
                        self.note_path(p, mi, Op::Eq, cx, usage);
                    }
                }
            }
            Predicate::Bare(path) => {
                // A single-segment bare atom may be a zero-arg named filter.
                if path.segments.len() == 1 {
                    if let Some(def) = cx.filters.get(&path.segments[0].node) {
                        self.expand_filter(def, mi, cx, usage, in_filters);
                        return;
                    }
                }
                self.note_path(path, mi, Op::Eq, cx, usage);
            }
            Predicate::FilterCall { name, args } => {
                if let Some(def) = cx.filters.get(&name.node) {
                    self.expand_filter(def, mi, cx, usage, in_filters);
                }
                for v in args {
                    if let Value::Path(p) = v {
                        self.note_path(p, mi, Op::Eq, cx, usage);
                    }
                }
            }
            Predicate::Raw(_) => self.opaque = true,
        }
    }

    fn expand_filter(
        &mut self,
        def: &NamedFilter,
        mi: usize,
        cx: &Cx,
        usage: &mut [Usage],
        in_filters: &mut Vec<String>,
    ) {
        if in_filters.iter().any(|n| n == &def.name.node) {
            return;
        }
        in_filters.push(def.name.node.clone());
        self.walk(&def.pred, mi, cx, usage, in_filters);
        in_filters.pop();
    }

    /// A constrained path: single-segment lands on the pattern (the root table);
    /// a relation reach records join demand per inverse hop and remote usage on
    /// the table the terminal column lives on.
    fn note_path(&mut self, path: &Path, mi: usize, op: Op, cx: &Cx, usage: &mut [Usage]) {
        if path.segments.len() == 1 {
            // Only fields that exist count; name errors were already reported.
            if cx.model(mi).member(&path.segments[0].node).is_some() {
                self.add(mi, op, &path.segments[0].node, usage);
            }
            return;
        }
        if let Some((tmi, terminal)) = trace(path, mi, cx, usage, &mut self.joins) {
            usage[tmi].touch(&terminal);
        }
    }

    /// Leading sort field: local single-segment paths land on the pattern;
    /// relation reaches only record demand/usage.
    fn note_sort(&mut self, terms: &[SortTerm], mi: usize, cx: &Cx, usage: &mut [Usage]) {
        for (i, t) in terms.iter().enumerate() {
            if t.path.segments.len() == 1 {
                let f = &t.path.segments[0].node;
                if cx.model(mi).member(f).is_some() {
                    if i == 0 && self.sort.is_none() {
                        self.sort = Some(f.clone());
                    }
                    usage[mi].touch(f);
                }
            } else if let Some((tmi, terminal)) = trace(&t.path, mi, cx, usage, &mut self.joins) {
                usage[tmi].touch(&terminal);
            }
        }
    }
}

/// Walk a multi-segment path recording join demand for every inverse hop (into both
/// the pooled usage and `joins`), and return where its terminal lands:
/// `(model idx, terminal field)`. Quiet — name errors were already reported by the
/// resolver.
fn trace(
    path: &Path,
    start: usize,
    cx: &Cx,
    usage: &mut [Usage],
    joins: &mut Vec<(usize, String)>,
) -> Option<(usize, String)> {
    let mut cur = start;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let mem = cx.model(cur).member(&seg.node)?;
        let last = i + 1 == n;
        match &mem.kind {
            MemberKind::Scalar { .. } => {
                return last.then(|| (cur, seg.node.clone()));
            }
            MemberKind::Forward { .. } => {
                // Forward joins land on the target's PK — always indexed.
                if last {
                    return Some((cur, seg.node.clone()));
                }
                cur = cx.find(mem.kind.target()?)?;
            }
            MemberKind::Inverse { target, via } => {
                let ti = cx.find(target)?;
                usage[ti].join(via);
                push_join(joins, ti, via);
                if last {
                    return Some((cur, seg.node.clone()));
                }
                cur = ti;
            }
        }
    }
    None
}

fn push_join(joins: &mut Vec<(usize, String)>, ti: usize, via: &str) {
    if !joins.iter().any(|(m, v)| *m == ti && v == via) {
        joins.push((ti, via.to_string()));
    }
}

// ---------- index availability ----------------------------------------------

/// Does declared structure already lead with `field`? PK/`(unique)` columns and
/// declared indexes count; a declared index may lead with the soft-delete column
/// (predicate-leading by hand) — that leading column is skipped.
fn covers(m: &RModel, field: &str) -> bool {
    m.is_unique(field) || m.indexes.iter().any(|i| lead(m, i) == Some(field))
}

/// The effective leading column of an index: the first column, skipping a leading
/// soft-delete column (it is equality-constrained on every generated query, so
/// the *next* column is what selects).
fn lead<'a>(m: &RModel, idx: &'a RIndex) -> Option<&'a str> {
    let mut cols = idx.columns.iter();
    let first = cols.next()?;
    match &m.soft_delete {
        Some(sd) if &sd.field == first => {
            cols.next().or(Some(first)).map(std::string::String::as_str)
        }
        _ => Some(first.as_str()),
    }
}

// ---------- E0260 / W0105 -----------------------------------------------------

fn check_pattern(pat: &Pattern, mi: usize, cx: &Cx, sink: &mut Sink) {
    let m = cx.model(mi);
    let leads = |field: &str| -> bool {
        m.is_unique(field) || m.indexes.iter().any(|i| lead(m, i) == Some(field))
    };

    // A filter with no leading index scans (unless it is opaque — then stay silent).
    let filter_ok = pat.opaque
        || if !pat.eq.is_empty() || !pat.range.is_empty() {
            pat.eq.iter().chain(&pat.range).any(|f| leads(f))
        } else if let (Some(sort), true) = (&pat.sort, pat.paginated) {
            // No filter at all: only a paginated list pays for its sort — an index on
            // the sort key turns a full filesort into an early-exit scan.
            leads(sort)
        } else {
            true // nothing an index could serve; nothing to check
        };

    // Join keys this query traverses that no declared `@index` covers.
    let uncovered: Vec<&(usize, String)> = pat
        .joins
        .iter()
        .filter(|(ti, via)| !covers(cx.model(*ti), via))
        .collect();

    let served = filter_ok && uncovered.is_empty();

    // The visible `unindexed(…)` opt-out satisfies both the filter and the join
    // requirement for this query; a stale one (the query is served) is W0105.
    match (&pat.annotation, served) {
        (Some(_), false) => return,
        (Some(u), true) => {
            sink.warn(
                code::STALE_UNINDEXED,
                u.span,
                format!(
                    "query `{}` is indexed — this `unindexed` annotation is stale; drop it",
                    pat.name
                ),
            );
            return;
        }
        (None, true) => return,
        (None, false) => {}
    }

    if !filter_ok {
        let wants: Vec<&str> = pat
            .eq
            .iter()
            .chain(&pat.range)
            .chain(&pat.sort)
            .map(std::string::String::as_str)
            .collect();
        let col = wants.first().copied().unwrap_or("");
        sink.error_fix(
            code::UNINDEXED_JOIN,
            pat.span,
            format!(
                "query `{}` will scan `{}`: no index leads with any of ({})",
                pat.name,
                m.name,
                wants.join(", ")
            ),
            "add an `@index`, or annotate `unindexed(max_rows: N)` / `unindexed(unsafe)`",
            m.name.clone(),
            format!("@index {col}"),
        );
    }
    for (ti, via) in uncovered {
        let target = cx.model(*ti);
        sink.error_fix(
            code::UNINDEXED_JOIN,
            pat.span,
            format!(
                "query `{}` joins `{}` through `{via}` with no covering index — the join will scan",
                pat.name, target.name
            ),
            "add an `@index`, or annotate `unindexed(max_rows: N)` / `unindexed(unsafe)`",
            target.name.clone(),
            format!("@index {via}"),
        );
    }
}

// ---------- W0104 -------------------------------------------------------------

fn lint_useless(ast: &Model, mi: usize, usage: &Usage, cx: &Cx, sink: &mut Sink) {
    let m = cx.model(mi);
    // Spans live on the AST; `validate_indexes` built `m.indexes` in member order.
    let decls = ast.members.iter().filter_map(|mem| match mem {
        Member::Index(i) => Some(i),
        _ => None,
    });
    for (idx, decl) in m.indexes.iter().zip(decls) {
        if idx.unique {
            continue; // a unique index is a constraint, not a perf structure
        }
        // An exotic index (`using <method>` / opaque `raw(…)`) serves access paths the
        // engine cannot see — a GIST/GIN lookup or an expression predicate reaches the
        // DB through raw, never through a modelled filter. It is the author's assertion.
        if idx.method.is_some() || idx.raw.is_some() {
            continue;
        }
        let Some(lead) = lead(m, idx) else { continue };
        // Leading with the soft-delete column alone: every generated query
        // filters it, so the index is used by construction.
        if m.soft_delete.as_ref().is_some_and(|sd| sd.field == lead) {
            continue;
        }
        if idx.columns.len() == 1 && m.is_unique(lead) {
            sink.warn(
                code::USELESS_INDEX,
                decl.span,
                format!(
                    "index on `{lead}` is unnecessary: its `(unique)` constraint already indexes it, so this only adds write cost — drop it"
                ),
            );
            continue;
        }
        let used =
            usage.cols.iter().any(|c| c == lead) || usage.join_fields.iter().any(|c| c == lead);
        if !used {
            sink.warn(
                code::USELESS_INDEX,
                decl.span,
                format!(
                    "index on `{lead}` is unnecessary: no query filters, sorts, or joins on it, so it only adds write cost — drop it or add the query that needs it"
                ),
            );
        }
    }
}

// ---------- shape traversal demand -------------------------------------------

/// Relation reaches in a shape body join at query time, so they create the same
/// join demand a predicate reach does. Nested sub-objects recurse into the target
/// model; the nest edge itself is a traversal. A `field -> Shape` reference expands
/// the named shape's body in place (guarded against reference cycles, which sema
/// rejects but this pass must still terminate on). Join reaches are recorded on
/// `joins` (the returning query's pattern) as well as the pooled `usage`.
fn shape_demand(
    body: &[ShapeField],
    mi: usize,
    cx: &Cx,
    usage: &mut [Usage],
    joins: &mut Vec<(usize, String)>,
) {
    shape_demand_in(body, mi, cx, usage, joins, &mut Vec::new());
}

fn shape_demand_in(
    body: &[ShapeField],
    mi: usize,
    cx: &Cx,
    usage: &mut [Usage],
    joins: &mut Vec<(usize, String)>,
    stack: &mut Vec<String>,
) {
    for f in body {
        match f {
            ShapeField::Bare(_) => {}
            ShapeField::Rename { value, .. } => {
                if let ShapeValue::Path(p) = value {
                    if p.segments.len() > 1 {
                        trace(p, mi, cx, usage, joins);
                    }
                }
            }
            ShapeField::Nest { field, body } => {
                if let Some(kind) = cx.model(mi).member(&field.node).map(|m| m.kind.clone()) {
                    if let MemberKind::Inverse { target, via } = &kind {
                        if let Some(ti) = cx.find(target) {
                            usage[ti].join(via);
                            push_join(joins, ti, via);
                        }
                    }
                    if let Some(ti) = kind.target().and_then(|t| cx.find(t)) {
                        shape_demand_in(body, ti, cx, usage, joins, stack);
                    }
                }
            }
            ShapeField::NestRef { field, shape } => {
                if stack.iter().any(|s| s == &shape.node) {
                    continue;
                }
                if let Some(kind) = cx.model(mi).member(&field.node).map(|m| m.kind.clone()) {
                    if let MemberKind::Inverse { target, via } = &kind {
                        if let Some(ti) = cx.find(target) {
                            usage[ti].join(via);
                            push_join(joins, ti, via);
                        }
                    }
                    if let (Some(ti), Some(body)) = (
                        kind.target().and_then(|t| cx.find(t)),
                        cx.shape_bodies.get(&shape.node).copied(),
                    ) {
                        stack.push(shape.node.clone());
                        shape_demand_in(body, ti, cx, usage, joins, stack);
                        stack.pop();
                    }
                }
            }
            // A flatten's first hop is a to-many inverse (into the junction) whose
            // correlated subquery filters on the junction's back FK — the same index
            // demand a to-many nest into the junction creates. Later forward hops join
            // on the target's (indexed) primary key. The body reaches the far model.
            ShapeField::Flatten { path, body, .. } => {
                let first = &path.segments[0];
                if let Some(MemberKind::Inverse { target, via }) =
                    cx.model(mi).member(&first.node).map(|m| &m.kind)
                {
                    if let Some(ti) = cx.find(target) {
                        usage[ti].join(via);
                        push_join(joins, ti, via);
                    }
                }
                let far = path.segments.iter().try_fold(mi, |cur, seg| {
                    cx.model(cur)
                        .member(&seg.node)
                        .and_then(|m| m.kind.target())
                        .and_then(|t| cx.find(t))
                });
                if let Some(far) = far {
                    shape_demand_in(body, far, cx, usage, joins, stack);
                }
            }
        }
    }
}
