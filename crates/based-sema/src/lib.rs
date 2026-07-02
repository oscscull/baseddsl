//! based-sema — semantic analysis over the closed declaration set.
//!
//! Builds one resolved schema from many parsed files, then checks it:
//!   * name + casing resolution (D7): `Order` <-> model `Order`, paths, inverses
//!   * implicit `id` column (D2); engine-managed timestamp roles
//!   * type checking: fields, shape paths, predicate operands, param bindings
//!   * the four query inferences (queries.md): verb, param type, filter, target
//!   * lints: nondeterministic sort, raw soft-delete gaps, get-not-keyed
//!
//! Output is a [`CheckedSchema`] (the IR seed for codegen) plus diagnostics.
//!
//! Pass order matters — models are built before anything references them:
//!   1. collect: gather decls by kind; report duplicate names.
//!   2. skeletons: one `RModel` per model (implicit `id`, columns, relations).
//!   3. validate (mut): relation targets, inverses, decorators, indexes, uniqueness.
//!   4. resolve exprs (read-only): `@scope`/`@sort` paths that traverse models.
//!   5. check shapes / queries / mutations / filters against the resolved models.

mod check;
mod ctx;
mod indexes;
mod ir;
mod model;
mod resolve;

pub use ir::*;

use based_ast::Decl;
use based_diagnostics::Diagnostic;
use resolve::Cx;
use std::collections::{HashMap, HashSet};

/// Resolve and check the whole declaration set (gathered from every `.bsl` file).
pub fn check(decls: &[Decl]) -> (CheckedSchema, Vec<Diagnostic>) {
    let mut sink = Sink::default();

    // 1. Collect declarations by kind.
    let mut models = Vec::new();
    let mut shapes = Vec::new();
    let mut queries = Vec::new();
    let mut mutations = Vec::new();
    let mut filters = Vec::new();
    for d in decls {
        match d {
            Decl::Model(m) => models.push(m),
            Decl::Shape(s) => shapes.push(s),
            Decl::Query(q) => queries.push(q),
            Decl::Mutation(m) => mutations.push(m),
            Decl::Filter(f) => filters.push(f),
        }
    }

    // Name tables + duplicate detection.
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, m) in models.iter().enumerate() {
        if index.contains_key(&m.name.node) {
            sink.error(
                code::DUP_MODEL,
                m.name.span,
                format!("duplicate model `{}`", m.name.node),
            );
        } else {
            index.insert(m.name.node.clone(), i);
        }
    }
    let mut shape_from: HashMap<String, String> = HashMap::new();
    for s in &shapes {
        // `full` is a per-model convention (D9), so duplicate `full` is allowed.
        if s.name.node != "full" && shape_from.contains_key(&s.name.node) {
            sink.error(
                code::DUP_SHAPE,
                s.name.span,
                format!("duplicate shape `{}`", s.name.node),
            );
        } else {
            shape_from.insert(s.name.node.clone(), s.from.node.clone());
        }
    }
    // Full filter defs (not just arity): the body is re-resolved against each
    // call-site model in the predicate checker.
    let mut filter_defs: HashMap<String, &based_ast::NamedFilter> = HashMap::new();
    for f in &filters {
        if filter_defs.contains_key(&f.name.node) {
            sink.error(
                code::DUP_FILTER,
                f.name.span,
                format!("duplicate filter `{}`", f.name.node),
            );
        } else {
            filter_defs.insert(f.name.node.clone(), *f);
        }
    }
    // Queries and mutations share the wire namespace (one route each, calling.md).
    let mut callable: HashSet<&str> = HashSet::new();
    for (name, span) in queries
        .iter()
        .map(|q| (&q.name.node, q.name.span))
        .chain(mutations.iter().map(|m| (&m.name.node, m.name.span)))
    {
        if !callable.insert(name.as_str()) {
            sink.error(
                code::DUP_CALLABLE,
                span,
                format!("duplicate query/mutation `{name}`"),
            );
        }
    }

    // 2. Skeletons, then 3. validate (needs &mut models + the name index).
    let mut rmodels: Vec<RModel> = models
        .iter()
        .map(|m| model::skeleton(m, &mut sink))
        .collect();
    for (mi, ast) in models.iter().enumerate() {
        model::validate(ast, mi, &mut rmodels, &index, &mut sink);
    }

    // 4/5. Everything from here reads the finished models through one context
    // (scoped so its borrow of `rmodels` ends before the inferred indexes land).
    let (rshapes, rqueries, rmutations, rfilters, inferred) = {
        let cx = Cx {
            models: &rmodels,
            index: &index,
            filters: &filter_defs,
            shapes: &shape_from,
        };

        for ast in &models {
            model::resolve_exprs(ast, &cx, &mut sink);
        }
        let rshapes: Vec<RShape> = shapes
            .iter()
            .filter_map(|s| check::check_shape(s, &cx, &mut sink))
            .collect();
        let rqueries: Vec<RQuery> = queries
            .iter()
            .filter_map(|q| check::check_query(q, &cx, &mut sink))
            .collect();
        let rmutations: Vec<RMutation> = mutations
            .iter()
            .filter_map(|m| check::check_mutation(m, &cx, &mut sink))
            .collect();
        let rfilters: Vec<RFilter> = filters
            .iter()
            .map(|f| check::check_filter(f, &cx, &mut sink))
            .collect();

        // 6. Index inference + lints (indexing.md, D15). Last on purpose: it
        // reasons over the *whole* resolved access layer (closed world, D5).
        let inferred = indexes::run(
            &models, &queries, &shapes, &mutations, &rqueries, &cx, &mut sink,
        );
        (rshapes, rqueries, rmutations, rfilters, inferred)
    };
    for (m, inf) in rmodels.iter_mut().zip(inferred) {
        m.inferred_indexes = inf;
    }

    // 7. `$ctx` coherence (D4/D5): each callable's inferred context requirement is
    // its own, but a field name must mean one type everywhere the caller's shared
    // context bag is read — closed world makes that a fact, not a guess.
    ctx::check_coherence(&rqueries, &rmutations, &mut sink);

    let schema = CheckedSchema {
        models: rmodels,
        shapes: rshapes,
        queries: rqueries,
        mutations: rmutations,
        filters: rfilters,
        model_index: index,
    };
    (schema, sink.diags)
}
