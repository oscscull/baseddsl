//! based-sema — semantic analysis over the closed declaration set.
//!
//! Builds one resolved schema from many parsed files, then checks it:
//!   * name + casing resolution : `Order` <-> model `Order`, paths, inverses
//!   * implicit `id` column ; engine-managed timestamp roles
//!   * type checking: fields, shape paths, predicate operands, param bindings
//!   * the four query inferences: verb, param type, filter, target
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
mod enums;
mod indexes;
mod ir;
mod model;
mod resolve;
mod scope;

pub use ir::*;

use based_ast::Decl;
use based_diagnostics::Diagnostic;
use resolve::Cx;
use std::collections::{HashMap, HashSet};

/// The target-specific half of the checks: everything that can only be judged once the
/// compile target is known. `dialect` is the canonical dialect name (`mariadb`,
/// `postgres`, `sqlite`). Run after [`check`], by whoever resolved the manifest.
///
/// * a per-dialect `raw({…})` map that omits the target has no type to emit (`E0270`)
/// * `@index … using <method>` on a target without that access method (`E0272`) — an
///   error at generation time, never a silently downgraded index
pub fn check_target(schema: &CheckedSchema, dialect: &str) -> Vec<Diagnostic> {
    let mut sink = Sink::default();
    for m in &schema.models {
        for mem in &m.members {
            if let Some(spec) = mem.kind.opaque() {
                raw_spec_covers(spec, dialect, &mut sink);
            }
        }
        for idx in &m.indexes {
            if let Some(spec) = &idx.raw {
                raw_spec_covers(spec, dialect, &mut sink);
            }
            let Some(method) = &idx.method else { continue };
            let Some(targets) = index_method_targets(method) else {
                continue; // unknown method: already reported dialect-free (E0272)
            };
            if !targets.contains(&dialect) {
                sink.error_note(
                    code::INDEX_METHOD,
                    idx.span,
                    format!("`using {method}` is not available on {dialect}"),
                    format!("`{method}` indexes exist on: {}", targets.join(", ")),
                );
            }
        }
    }
    sink.diags
}

/// The FK-convention half of the checks: the divergence-reason rule, which can only be
/// judged once the project's `foreign_keys` convention is known. Run after [`check`], by
/// whoever resolved the manifest — the CLI with the manifest value, the LSP with the
/// resolved project value (the dialect-free [`check`] cannot decide divergence alone).
///
/// A reason string is required exactly when a decorator flips FK **presence** against the
/// convention: `@fk` under `foreign_keys = "none"` (adds an FK) or `@no_fk` under
/// `"all"` (removes one) — a missing reason is `E0295`. A decorator that merely restates
/// the convention (a `@no_fk` under `"none"`, a bare `@fk` under `"all"`) has no effect
/// and earns the `W0110` redundancy lint. Referential actions never trigger a reason on
/// their own — only flipping presence does.
pub fn check_foreign_keys(schema: &CheckedSchema, fks: ForeignKeys) -> Vec<Diagnostic> {
    let mut sink = Sink::default();
    for m in &schema.models {
        // Model-level `@no_fk` (whole-table opt-out).
        if m.no_fk {
            let span = m.no_fk_span.unwrap_or(m.span);
            match fks {
                ForeignKeys::All if m.no_fk_reason.is_none() => sink.error(
                    code::FK_DIVERGE_REASON,
                    span,
                    format!(
                        "`@no_fk` on `{}` drops the FK constraints your `foreign_keys = \"all\"` \
                         convention would create — add a reason: `@no_fk(\"why\")`",
                        m.name
                    ),
                ),
                ForeignKeys::None => sink.warn(
                    code::FK_REDUNDANT,
                    span,
                    format!(
                        "`@no_fk` on `{}` restates `foreign_keys = \"none\"` (no FK either way) — remove it",
                        m.name
                    ),
                ),
                ForeignKeys::All => {}
            }
        }
        for mem in &m.members {
            let MemberKind::Forward {
                fk, custom_join, ..
            } = &mem.kind
            else {
                continue;
            };
            // A custom-join edge owns no FK column — already `E0291`; nothing to weigh here.
            if *custom_join {
                continue;
            }
            if fk.fk {
                let span = fk.fk_span.unwrap_or(mem.span);
                let has_actions = fk.on_delete.is_some() || fk.on_update.is_some();
                match fks {
                    ForeignKeys::None if fk.fk_reason.is_none() => sink.error(
                        code::FK_DIVERGE_REASON,
                        span,
                        format!(
                            "`@fk` on `{}.{}` adds an FK your `foreign_keys = \"none\"` convention \
                             omits — add a reason: `@fk(\"why\"{})`",
                            m.name,
                            mem.name,
                            if has_actions { ", on_delete: …" } else { "" }
                        ),
                    ),
                    // Under `all` a bare `@fk` just restates the convention; one carrying
                    // actions is legitimately refining a present FK (no lint).
                    ForeignKeys::All if !has_actions => sink.warn(
                        code::FK_REDUNDANT,
                        span,
                        format!(
                            "`@fk` on `{}.{}` restates `foreign_keys = \"all\"` (already an FK) — \
                             drop it, or add an `on_delete`/`on_update` action",
                            m.name, mem.name
                        ),
                    ),
                    ForeignKeys::None | ForeignKeys::All => {}
                }
            }
            if fk.no_fk {
                let span = fk.no_fk_span.unwrap_or(mem.span);
                match fks {
                    ForeignKeys::All if fk.no_fk_reason.is_none() => sink.error(
                        code::FK_DIVERGE_REASON,
                        span,
                        format!(
                            "`@no_fk` on `{}.{}` drops the FK your `foreign_keys = \"all\"` \
                             convention creates — add a reason: `@no_fk(\"why\")`",
                            m.name, mem.name
                        ),
                    ),
                    ForeignKeys::None => sink.warn(
                        code::FK_REDUNDANT,
                        span,
                        format!(
                            "`@no_fk` on `{}.{}` restates `foreign_keys = \"none\"` (no FK either way) — remove it",
                            m.name, mem.name
                        ),
                    ),
                    ForeignKeys::All => {}
                }
            }
        }
    }
    sink.diags
}

fn raw_spec_covers(spec: &based_ast::RawSpec, dialect: &str, sink: &mut Sink) {
    if spec.for_dialect(dialect).is_none() {
        sink.error_note(
            code::RAW_TYPE_DIALECT,
            spec.span,
            format!("this `raw({{…}})` map has no entry for the compile target {dialect}"),
            format!("add `{dialect}: \"…\"`, or use the bare `raw(\"…\")` form for all targets"),
        );
    }
}

/// The declaration set bucketed by kind — pass 1's output, borrowed from the input slice.
#[derive(Default)]
struct Decls<'a> {
    models: Vec<&'a based_ast::Model>,
    shapes: Vec<&'a based_ast::Shape>,
    scopes: Vec<&'a based_ast::ScopeDecl>,
    enums: Vec<&'a based_ast::EnumDecl>,
    queries: Vec<&'a based_ast::Query>,
    mutations: Vec<&'a based_ast::Mutation>,
    filters: Vec<&'a based_ast::NamedFilter>,
}

impl<'a> Decls<'a> {
    fn collect(decls: &'a [Decl]) -> Self {
        let mut out = Self::default();
        for d in decls {
            match d {
                Decl::Model(m) => out.models.push(m),
                Decl::Shape(s) => out.shapes.push(s),
                Decl::Scope(s) => out.scopes.push(s),
                Decl::Enum(e) => out.enums.push(e),
                Decl::Query(q) => out.queries.push(q),
                Decl::Mutation(m) => out.mutations.push(m),
                Decl::Filter(f) => out.filters.push(f),
            }
        }
        out
    }
}

/// The name tables every later pass reads, with duplicates already reported.
struct Names<'a> {
    /// Model name -> its position in `Decls::models` (and so in the `RModel` vec).
    index: HashMap<String, usize>,
    /// Shape name -> the model it projects.
    shape_from: HashMap<String, String>,
    /// Shape name -> its body, so `$ctx` collection can walk a return shape's relation
    /// reaches to find joined scoped models. Last write wins on a duplicate name
    /// (already reported); the collector only reads it.
    shape_bodies: HashMap<String, &'a [based_ast::ShapeField]>,
    /// Full filter defs, not just arity: the body is re-resolved against each call-site
    /// model in the predicate checker.
    filters: HashMap<String, &'a based_ast::NamedFilter>,
}

impl<'a> Names<'a> {
    fn build(decls: &Decls<'a>, sink: &mut Sink) -> Self {
        let mut index: HashMap<String, usize> = HashMap::new();
        for (i, m) in decls.models.iter().enumerate() {
            // First declaration wins the index; later ones are reported, not recorded.
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
        let mut shape_bodies: HashMap<String, &[based_ast::ShapeField]> = HashMap::new();
        for s in &decls.shapes {
            // `full` is a per-model convention, so duplicate `full` is allowed.
            if s.name.node != "full" && shape_from.contains_key(&s.name.node) {
                sink.error(
                    code::DUP_SHAPE,
                    s.name.span,
                    format!("duplicate shape `{}`", s.name.node),
                );
            } else {
                shape_from.insert(s.name.node.clone(), s.from.node.clone());
            }
            shape_bodies.insert(s.name.node.clone(), s.body.as_slice());
        }
        let mut filters: HashMap<String, &based_ast::NamedFilter> = HashMap::new();
        for f in &decls.filters {
            if filters.contains_key(&f.name.node) {
                sink.error(
                    code::DUP_FILTER,
                    f.name.span,
                    format!("duplicate filter `{}`", f.name.node),
                );
            } else {
                filters.insert(f.name.node.clone(), *f);
            }
        }
        // Queries and mutations share the wire namespace (one route each).
        let mut callable: HashSet<&str> = HashSet::new();
        for (name, span) in decls
            .queries
            .iter()
            .map(|q| (&q.name.node, q.name.span))
            .chain(decls.mutations.iter().map(|m| (&m.name.node, m.name.span)))
        {
            if !callable.insert(name.as_str()) {
                sink.error(
                    code::DUP_CALLABLE,
                    span,
                    format!("duplicate query/mutation `{name}`"),
                );
            }
        }
        Self {
            index,
            shape_from,
            shape_bodies,
            filters,
        }
    }

    /// Every type name already spoken for, which enum resolution must not collide with.
    fn type_names(&self, decls: &Decls) -> HashSet<String> {
        let mut taken: HashSet<String> = self.index.keys().cloned().collect();
        taken.extend(self.shape_from.keys().cloned());
        taken.extend(decls.scopes.iter().map(|s| s.name.node.clone()));
        taken
    }
}

/// Passes 2–3b: one `RModel` per model, validated, with named scopes attached.
fn build_models(
    decls: &Decls,
    names: &Names,
    enums: &[REnum],
    sink: &mut Sink,
) -> (Vec<RModel>, Vec<RScope>, HashMap<String, usize>) {
    // Enum name -> its inferred kind, so a field's classification picks the storage type
    // (an int enum is an integer column; a string enum is text).
    let enum_kinds: HashMap<String, EnumKind> =
        enums.iter().map(|e| (e.name.clone(), e.kind)).collect();

    let mut rmodels: Vec<RModel> = decls
        .models
        .iter()
        .map(|m| model::skeleton(m, &enum_kinds, sink))
        .collect();
    for (mi, ast) in decls.models.iter().enumerate() {
        model::validate(ast, mi, &mut rmodels, &names.index, sink);
    }

    // Named scopes: resolve the `scope` decls, then attach each model's `@scope Name`
    // refs (E0183/E0184) + synthesize the injected predicate.
    let (rscopes, scope_index) = scope::resolve_decls(&decls.scopes, &names.index, sink);
    scope::attach_models(&decls.models, &mut rmodels, &rscopes, &scope_index, sink);
    (rmodels, rscopes, scope_index)
}

/// Passes 4–6: everything that reads the finished models through one context.
fn check_access_layer(
    decls: &Decls,
    cx: &Cx,
    sink: &mut Sink,
) -> (Vec<RShape>, Vec<RQuery>, Vec<RMutation>, Vec<RFilter>) {
    for ast in &decls.models {
        model::resolve_exprs(ast, cx, sink);
    }
    // Enum-typed field defaults (a `default <variant>` must name a member).
    enums::check_field_defaults(cx, sink);
    let rshapes: Vec<RShape> = decls
        .shapes
        .iter()
        .filter_map(|s| check::check_shape(s, cx, sink))
        .collect();
    let rqueries: Vec<RQuery> = decls
        .queries
        .iter()
        .filter_map(|q| check::check_query(q, cx, sink))
        .collect();
    let rmutations: Vec<RMutation> = decls
        .mutations
        .iter()
        .filter_map(|m| check::check_mutation(m, cx, sink))
        .collect();
    let rfilters: Vec<RFilter> = decls
        .filters
        .iter()
        .map(|f| check::check_filter(f, cx, sink))
        .collect();

    // Index requirement checks + lints. Last on purpose: it reasons over the *whole*
    // resolved access layer (closed world).
    indexes::run(
        &decls.models,
        &decls.queries,
        &decls.shapes,
        &decls.mutations,
        &rqueries,
        &rmutations,
        cx,
        sink,
    );
    (rshapes, rqueries, rmutations, rfilters)
}

/// Resolve and check the whole declaration set (gathered from every `.bsl` file).
pub fn check(decls: &[Decl]) -> (CheckedSchema, Vec<Diagnostic>) {
    let mut sink = Sink::default();

    let decls = Decls::collect(decls);
    let names = Names::build(&decls, &mut sink);

    // Enum decls share the type-name namespace with models/shapes/scopes; resolve them
    // now (before skeletons) so a field typed by an enum name classifies as a scalar
    // column, not a relation.
    let taken = names.type_names(&decls);
    let (renums, enum_index) = enums::resolve_decls(&decls.enums, &taken, &mut sink);

    let (rmodels, rscopes, scope_index) = build_models(&decls, &names, &renums, &mut sink);

    let (rshapes, rqueries, rmutations, rfilters) = check_access_layer(
        &decls,
        &Cx {
            models: &rmodels,
            index: &names.index,
            filters: &names.filters,
            shapes: &names.shape_from,
            shape_bodies: &names.shape_bodies,
            scopes: &rscopes,
            scope_index: &scope_index,
            enums: &renums,
            enum_index: &enum_index,
        },
        &mut sink,
    );

    // `$ctx` coherence: each callable's inferred context requirement is its own, but a
    // field name must mean one type everywhere the caller's shared context bag is read —
    // closed world makes that a fact, not a guess.
    ctx::check_coherence(&rqueries, &rmutations, &mut sink);

    let schema = CheckedSchema {
        models: rmodels,
        shapes: rshapes,
        scopes: rscopes,
        enums: renums,
        queries: rqueries,
        mutations: rmutations,
        filters: rfilters,
        model_index: names.index,
        scope_index,
        enum_index,
    };
    (schema, sink.diags)
}
