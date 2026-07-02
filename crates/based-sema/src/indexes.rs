//! Index inference + the bidirectional index lints (indexing.md, D15).
//!
//! Runs after every query/shape/mutation is resolved, because the whole point is
//! closed-world reasoning: the access layer *is* the set of generated SQL, so the
//! engine can know which indexes queries need and which declared indexes nothing
//! uses.
//!
//! Three outputs:
//!   * **Inferred baseline** (per model): an index on the FK column of every
//!     inverse edge some query/shape actually traverses — the join keys, the one
//!     class of index that is unambiguously right to auto-create. Deduped against
//!     declared structure; emitted by DDL codegen (predicate-leading there).
//!   * **W0103 `unindexed`** (missing-index): a query — or a mutation's `update`/
//!     `delete`/`restore` `where`, which scans the same way — whose filter pattern
//!     no available index serves. Satisfied by an `@index` or by the
//!     `unindexed(max_rows: N)` / `unindexed(unsafe)` annotation (a query-body
//!     clause; a bulk write has no such clause, so it simply shows). Filter-path
//!     indexes are *not* auto-created (whether one is worth its write tax is a
//!     human call — principle 8: shown, not written); this lint is how they show.
//!   * **W0104 `useless-index`**: a declared non-unique index nothing in the access
//!     layer (queries *and* mutation `where`s) filters, sorts, or joins on — pure
//!     write tax. (Unique indexes are constraints, not perf, so they are exempt.)
//!     W0105 flags the inverse staleness: an `unindexed` annotation on a query that
//!     turns out indexed.
//!
//! The pattern model is coarse on purpose (same spirit as operand typing): eq/
//! range columns off the conjunctive spine, first-column index matching. It aims
//! to catch "this query has no index at all", not to re-derive the planner. A
//! predicate with an `or` or a raw atom is opaque — W0103 stays silent rather
//! than guessing (precision over recall; a warn lint must not cry wolf).

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;
use crate::resolve::Cx;

/// Run inference + lints. Returns the inferred baseline, indexed like `cx.models`
/// (the caller owns `models` mutably only after `cx` is dropped).
pub fn run(
    models_ast: &[&Model],
    queries_ast: &[&Query],
    shapes_ast: &[&Shape],
    mutations_ast: &[&Mutation],
    rqueries: &[RQuery],
    cx: &Cx,
    sink: &mut Sink,
) -> Vec<Vec<RIndex>> {
    let ast_by_name: HashMap<&str, &Query> = queries_ast
        .iter()
        .map(|q| (q.name.node.as_str(), *q))
        .collect();
    let shape_by_name: HashMap<&str, &Shape> = shapes_ast
        .iter()
        .map(|s| (s.name.node.as_str(), *s))
        .collect();

    // ---- collect: one pattern per query, one usage/demand pool per model ----
    let mut usage: Vec<Usage> = cx.models.iter().map(|_| Usage::default()).collect();
    let mut patterns: Vec<(usize, Pattern)> = Vec::new(); // (model idx, pattern)

    for rq in rqueries {
        let Some(mi) = cx.find(&rq.target) else {
            continue;
        };
        let Some(ast) = ast_by_name.get(rq.name.as_str()).copied() else {
            continue;
        };
        let pat = query_pattern(ast, rq, mi, cx, &mut usage);
        patterns.push((mi, pat));

        // A shape only creates traversal demand through a query that returns it.
        if let Some(shape) = rq.ret_shape.as_deref().and_then(|s| shape_by_name.get(s)) {
            shape_demand(&shape.body, mi, cx, &mut usage);
        }
    }

    // A mutation's `update`/`delete`/`restore` `where` scans exactly like a query's
    // — feed each into the same pool so W0103 flags an unindexed bulk write and
    // W0104 counts a column a mutation filters on as used.
    for mu in mutations_ast {
        for stmt in &mu.body {
            collect_write(stmt, &mu.name.node, cx, &mut usage, &mut patterns);
        }
    }

    // ---- inferred baseline: traversed inverse-edge FKs, minus declared -------
    let inferred: Vec<Vec<RIndex>> = usage
        .iter()
        .enumerate()
        .map(|(mi, u)| {
            let m = cx.model(mi);
            u.join_fields
                .iter()
                .filter(|f| !covers(m, f))
                .map(|f| RIndex {
                    columns: vec![f.clone()],
                    unique: false,
                })
                .collect()
        })
        .collect();

    // ---- W0103 / W0105: pattern vs available indexes ------------------------
    for (mi, pat) in &patterns {
        check_pattern(pat, *mi, &inferred[*mi], cx, sink);
    }

    // ---- W0104: declared indexes nothing uses --------------------------------
    for (mi, ast) in models_ast.iter().enumerate() {
        lint_useless(ast, mi, &usage[mi], cx, sink);
    }

    inferred
}

// ---------- per-model usage pool -------------------------------------------

/// Everything the access layer touches on one model, pooled across queries.
/// Feeds the useless-index lint (broad: a column mentioned anywhere counts as
/// "used", so W0104 under-fires rather than over-fires) and the join-key
/// inference (`join_fields`).
#[derive(Default)]
struct Usage {
    /// Columns filtered (eq or range) or sorted on, by any query — including
    /// filters that land here through a relation reach from another model.
    cols: Vec<String>,
    /// Forward fields whose FK column a traversed inverse edge joins through
    /// (`via` of each traversed `Inverse`) — the inferred-baseline seed.
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
    paginated: bool,
    /// `or` / raw atom seen: the pattern is beyond first-column reasoning, so
    /// W0103 stays silent instead of guessing.
    opaque: bool,
    annotation: Option<Unindexed>,
}

fn query_pattern(q: &Query, rq: &RQuery, mi: usize, cx: &Cx, usage: &mut [Usage]) -> Pattern {
    let mut pat = Pattern {
        name: rq.name.clone(),
        span: rq.span,
        eq: Vec::new(),
        range: Vec::new(),
        sort: None,
        paginated: rq.paginated,
        opaque: false,
        annotation: None,
    };

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
        QueryBody::Bare => &[],
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
        }
    }
    // Sort cascade (sorting.md): the model default applies when the query is bare.
    if !has_order && !cx.model(mi).sort.is_empty() {
        let terms = cx.model(mi).sort.clone();
        pat.note_sort(&terms, mi, cx, usage);
    }
    // `@scope` rides into every query on the model (auth.md), filters included.
    if let Some(scope) = cx.model(mi).scope.clone() {
        pat.walk(&scope, mi, cx, usage, &mut Vec::new());
    }
    pat
}

/// Turn a write statement's `where` into a root-table access pattern (recursing
/// through `tx`). `create` has no `where`; `raw` has no bound model. A mutation
/// carries no `unindexed(…)` clause (that is a query-body annotation), so a
/// scanning bulk write can't be annotated away — it just shows.
fn collect_write(
    stmt: &WriteStmt,
    mut_name: &str,
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
                collect_write(s, mut_name, cx, usage, patterns);
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
        write_pattern(mut_name, model.span, where_, mi, cx, usage),
    ));
}

/// The access pattern of one write `where`: its conjunctive eq/range spine plus the
/// model's `@scope` (which rides into every write, auth.md). No sort or pagination
/// applies to a write, and there is no annotation to suppress it.
fn write_pattern(
    name: &str,
    span: Span,
    where_: &Predicate,
    mi: usize,
    cx: &Cx,
    usage: &mut [Usage],
) -> Pattern {
    let mut pat = Pattern {
        name: name.to_string(),
        span,
        eq: Vec::new(),
        range: Vec::new(),
        sort: None,
        paginated: false,
        opaque: false,
        annotation: None,
    };
    pat.walk(where_, mi, cx, usage, &mut Vec::new());
    if let Some(scope) = cx.model(mi).scope.clone() {
        pat.walk(&scope, mi, cx, usage, &mut Vec::new());
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
    /// the call-site model (D14), guarded against self-reference.
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
        if let Some((tmi, terminal)) = trace(path, mi, cx, usage) {
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
            } else if let Some((tmi, terminal)) = trace(&t.path, mi, cx, usage) {
                usage[tmi].touch(&terminal);
            }
        }
    }
}

/// Walk a multi-segment path recording join demand for every inverse hop, and
/// return where its terminal lands: `(model idx, terminal field)`. Quiet — name
/// errors were already reported by the resolver.
fn trace(path: &Path, start: usize, cx: &Cx, usage: &mut [Usage]) -> Option<(usize, String)> {
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
                if last {
                    return Some((cur, seg.node.clone()));
                }
                cur = ti;
            }
        }
    }
    None
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
        Some(sd) if &sd.field == first => cols.next().or(Some(first)).map(|s| s.as_str()),
        _ => Some(first.as_str()),
    }
}

// ---------- W0103 / W0105 -----------------------------------------------------

fn check_pattern(pat: &Pattern, mi: usize, inferred: &[RIndex], cx: &Cx, sink: &mut Sink) {
    if pat.opaque {
        return; // beyond first-column reasoning; stay silent rather than guess
    }
    let m = cx.model(mi);
    let leads = |field: &str| -> bool {
        m.is_unique(field)
            || m.indexes.iter().any(|i| lead(m, i) == Some(field))
            || inferred.iter().any(|i| lead(m, i) == Some(field))
    };

    let served = if !pat.eq.is_empty() || !pat.range.is_empty() {
        pat.eq.iter().chain(&pat.range).any(|f| leads(f))
    } else if let (Some(sort), true) = (&pat.sort, pat.paginated) {
        // No filter at all: only a paginated list pays for its sort — an index on
        // the sort key turns a full filesort into an early-exit scan.
        leads(sort)
    } else {
        true // nothing an index could serve; nothing to lint
    };

    match (&pat.annotation, served) {
        (None, false) => {
            let wants: Vec<&str> = pat
                .eq
                .iter()
                .chain(&pat.range)
                .chain(&pat.sort)
                .map(|s| s.as_str())
                .collect();
            sink.warn_note(
                code::UNINDEXED,
                pat.span,
                format!(
                    "query `{}` will scan `{}`: no index leads with any of ({})",
                    pat.name,
                    m.name,
                    wants.join(", ")
                ),
                "add an `@index`, or annotate `unindexed(max_rows: N)` / `unindexed(unsafe)`",
            );
        }
        (Some(u), true) => sink.warn(
            code::STALE_UNINDEXED,
            u.span,
            format!(
                "query `{}` is indexed — this `unindexed` annotation is stale; drop it",
                pat.name
            ),
        ),
        _ => {}
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
                    "index on `{lead}` duplicates its UNIQUE constraint — pure write tax; drop it"
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
                    "no query filters, sorts, or joins on `{lead}` — this index is pure write tax; drop it or add the query that needs it"
                ),
            );
        }
    }
}

// ---------- shape traversal demand -------------------------------------------

/// Relation reaches in a shape body join at query time, so they create the same
/// join demand a predicate reach does. Nested sub-objects recurse into the target
/// model; the nest edge itself is a traversal.
fn shape_demand(body: &[ShapeField], mi: usize, cx: &Cx, usage: &mut [Usage]) {
    for f in body {
        match f {
            ShapeField::Bare(_) => {}
            ShapeField::Rename { value, .. } => {
                if let ShapeValue::Path(p) = value {
                    if p.segments.len() > 1 {
                        trace(p, mi, cx, usage);
                    }
                }
            }
            ShapeField::Nest { field, body } => {
                if let Some(kind) = cx.model(mi).member(&field.node).map(|m| m.kind.clone()) {
                    if let MemberKind::Inverse { target, via } = &kind {
                        if let Some(ti) = cx.find(target) {
                            usage[ti].join(via);
                        }
                    }
                    if let Some(ti) = kind.target().and_then(|t| cx.find(t)) {
                        shape_demand(body, ti, cx, usage);
                    }
                }
            }
        }
    }
}
