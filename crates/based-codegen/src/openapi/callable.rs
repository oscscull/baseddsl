//! The per-callable IR the emitter builds from the schema + AST — mirrors the client
//! emitter's `Callable` so the two agree on routes, schema names, and pagination.

use super::*;
use std::collections::HashMap;

/// What a single query/mutation contributes to the spec: its path, its input schema
/// name, and its resolved output type. Mirrors the client emitter's `Callable` so the
/// two stay in lockstep (same routes, same schema names).
pub(crate) struct Callable<'a> {
    /// signature name (also the path tail) — already snake_case.
    pub(crate) name: &'a str,
    /// `/q/<name>` for a query, `/m/<name>` for a mutation.
    pub(crate) route: String,
    pub(crate) params: &'a [Param],
    /// model the params resolve against (query target / mutation return model);
    /// `None` when it could not be resolved.
    pub(crate) root: Option<&'a RModel>,
    /// Each param resolved to the entity (model) whose primary key it carries on the wire —
    /// its annotation, or its binding / `= $param` comparison against an FK / id. Absent for
    /// plain scalar / enum / shape params. Sourced from `based_sema` so the schema matches
    /// the client type and the runtime coercion (a `serial` reference is an integer, not uuid).
    pub(crate) param_entities: HashMap<String, String>,
    /// The response schema for the `200` body, as a JSON-Schema value.
    pub(crate) response: Value,
    /// The output *object* schema to register in `components.schemas`, deduped by
    /// name across callables (a shape shared by two callables is one schema).
    pub(crate) out_schema: OutSchema,
    /// Whether this is a mutation (drives the summary + `MutationResult` fallback).
    pub(crate) is_mutation: bool,
    /// A `-> stream` query: the `200` body is `application/x-ndjson` (one
    /// row/done/error envelope per line), not a JSON document.
    pub(crate) stream: bool,
    /// How this callable paginates, driving the extra input property:
    /// a keyset page a `cursor` string, an offset page an `offset` integer.
    pub(crate) page: PageInput,
}

/// How a callable paginates, driving its extra request-body property.
/// Mirrors the client emitter's enum so the two request surfaces stay in lockstep.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageInput {
    None,
    Keyset,
    Offset,
}

/// A named output object schema: a shape projection or a bare-model row.
pub(crate) struct OutSchema {
    pub(crate) name: String,
    /// `(field name, JSON-Schema value, required)` per projected column.
    pub(crate) fields: Vec<(String, Value, bool)>,
    /// A pure update/delete has no declared-shape row to project, so it emits no
    /// object schema — its `200` is the shared `MutationResult`. `true` skips it.
    pub(crate) is_result_fallback: bool,
    /// Named shapes this schema's body references via `field -> Shape` (recursively),
    /// each a full schema of its own — registered in `components.schemas` so the
    /// property `$ref`s resolve, deduped by name across callables.
    pub(crate) nested: Vec<Self>,
}

/// Build the callable descriptors from the checked schema + AST — the OpenAPI twin of
/// the client emitter's `collect`, so the two agree on routes and schema names.
pub(crate) fn collect<'a>(schema: &'a CheckedSchema, decls: &'a [Decl]) -> Vec<Callable<'a>> {
    let queries: HashMap<&str, &RQuery> = schema
        .queries
        .iter()
        .map(|q| (q.name.as_str(), q))
        .collect();
    let mutations: HashMap<&str, &RMutation> = schema
        .mutations
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();

    let mut out = Vec::new();
    for decl in decls {
        match decl {
            Decl::Query(q) => {
                if let Some(c) = query_callable(schema, decls, &queries, q) {
                    out.push(c);
                }
            }
            Decl::Mutation(m) => {
                if let Some(c) = mutation_callable(schema, decls, &mutations, m) {
                    out.push(c);
                }
            }
            _ => {}
        }
    }
    out
}

/// One query's descriptor: its target model types the params + output shape, and its
/// `200` is a single row / array / `Page` / NDJSON stream.
fn query_callable<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    queries: &HashMap<&str, &'a RQuery>,
    q: &'a Query,
) -> Option<Callable<'a>> {
    let rq = *queries.get(q.name.node.as_str())?;
    let root = schema.model(&rq.target);
    let os = out_schema(schema, decls, &q.ret, root, false);
    Some(Callable {
        name: &q.name.node,
        route: format!("/q/{}", q.name.node),
        params: &q.params,
        root,
        param_entities: based_sema::query_param_entities(schema, root, &q.params),
        response: query_response(rq, &os, page_with_count(q)),
        out_schema: os,
        is_mutation: false,
        stream: rq.stream,
        page: page_input(q),
    })
}

/// One mutation's descriptor: an `-> ok` mutation answers with the shared empty `Ack`;
/// a create-returning mutation advertises its re-selected shape; a pure update/delete
/// falls back to the `{ id }` `MutationResult`.
fn mutation_callable<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    mutations: &HashMap<&str, &'a RMutation>,
    m: &'a Mutation,
) -> Option<Callable<'a>> {
    let rm = *mutations.get(m.name.node.as_str())?;
    // `-> ok` names no shape/model: the primary written model (sema's `ret_model`)
    // types the params; the `200` is the shared empty `Ack`.
    let root = if rm.ack {
        schema.model(&rm.ret_model)
    } else {
        schema.model(&m.ret.ty.node).or_else(|| {
            schema
                .shapes
                .iter()
                .find(|s| s.name == m.ret.ty.node)
                .and_then(|s| schema.model(&s.from))
        })
    };
    // A mutation only advertises its declared shape when it re-selects one
    // (a create-returning mutation); a pure update/delete responds
    // `{ id }`, so it emits no object schema and points at `MutationResult`.
    let has_reselect = rm.ret_shape.is_some() || schema.model(&m.ret.ty.node).is_some();
    let os = out_schema(schema, decls, &m.ret, root, rm.ack || !has_reselect);
    let response = if rm.ack {
        schema_ref("Ack")
    } else {
        mutation_response(m, &os)
    };
    Some(Callable {
        name: &m.name.node,
        route: format!("/m/{}", m.name.node),
        params: &m.params,
        root,
        param_entities: based_sema::mutation_param_entities(schema, m),
        response,
        out_schema: os,
        is_mutation: true,
        stream: false,
        page: PageInput::None,
    })
}

/// How a query paginates, for its request-body page-control property.
fn page_input(q: &Query) -> PageInput {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => return PageInput::None,
    };
    clauses
        .iter()
        .find_map(|c| match c {
            Clause::Page(p) if p.offset => Some(PageInput::Offset),
            Clause::Page(_) => Some(PageInput::Keyset),
            _ => None,
        })
        .unwrap_or(PageInput::None)
}

/// Whether a query's `page` clause declares `with count` — its `Page` envelope then
/// also carries `total`.
fn page_with_count(q: &Query) -> bool {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => return false,
    };
    clauses
        .iter()
        .any(|c| matches!(c, Clause::Page(p) if p.with_count))
}
