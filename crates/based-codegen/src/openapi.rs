//! OpenAPI codegen (`based gen openapi`): a `CheckedSchema` -> a single OpenAPI 3.1
//! document describing the JSON/HTTP wire `serve::dispatch` already serves.
//!
//! ## Why an OpenAPI spec, not N per-language emitters
//! One machine-readable contract that `openapi-generator` (or similar) turns into a typed
//! client in any language — polyglot is one emitter, not N. The **Rust** client stays
//! hand-emitted ([`client`]) because it is the in-process `Transport` path (tighter than a
//! generated HTTP stub); everything else falls out of this spec.
//!
//! [`client`]: crate::client
//!
//! ## What we emit
//! One `paths` entry per callable (`POST /q/<name>` for a query, `POST /m/<name>` for
//! a mutation) and one `components.schemas` entry per input/output type — the exact
//! surface the client emitter builds, re-projected as JSON Schema. The emitter
//! *documents* the existing wire; it invents no new endpoint or shape.
//!
//! - **Request body** = the input schema (one property per signature param, required
//!   unless the param has a `(default)` or is optional).
//! - **Responses**: `200` with the response schema (the return wrapper — a bare `T`,
//!   an array, or the `Page<T>` / `{ id }` envelope) and the shared error responses
//!   (`{ "error": { code, message } }`, the envelope `serve::dispatch` returns).
//! - **`$ctx` is not a body param.** It rides the `X-Based-Context` header,
//!   so the spec models it as a header parameter referencing a shared component, never
//!   a request-body field.
//!
//! ## Type mapping (re-projected to JSON Schema)
//! `text`/`uuid`/`timestamp`/`date` -> `string` (`uuid`/`date-time`/`date` formats
//! where they carry one), `int` -> `integer`, `bool` -> `boolean`, `json` -> an open
//! object, a relation FK -> a `uuid` string (the wire carries the id). `optional`
//! -> the property is not `required`; a to-many scalar -> an `array`.
//!
//! ## Shape projection
//! - A to-**one** nested sub-object (`buyer { … }`) is emitted as an inline nested
//!   object schema; a to-**many** nest (`items { … }`) as an `array` of that object
//!   schema — both matching the client + SQL sides.
//! - A `raw`…`` shape field has no statically known type -> the open-object `Json`.
//! - A mutation with a declared return shape advertises it; one with no declared shape
//!   (e.g. a bare-`{ id }` write) responds with the shared `MutationResult` `{ id }`.

use based_ast::*;
use based_sema::{CheckedSchema, MemberKind, RModel, RMutation, RQuery};
use serde_json::{json, Map, Value};

/// Render the whole schema as an OpenAPI 3.1 document (pretty-printed JSON).
pub fn openapi(schema: &CheckedSchema, decls: &[Decl]) -> String {
    let doc = document(schema, decls);
    // Pretty JSON keeps the artifact reviewable; a
    // trailing newline matches the SQL/client emitters.
    let mut s = serde_json::to_string_pretty(&doc).expect("openapi document serializes");
    s.push('\n');
    s
}

// ---------- the resolved surface a callable contributes --------------------

/// What a single query/mutation contributes to the spec: its path, its input schema
/// name, and its resolved output type. Mirrors the client emitter's `Callable` so the
/// two stay in lockstep (same routes, same schema names).
struct Callable<'a> {
    /// signature name (also the path tail) — already snake_case.
    name: &'a str,
    /// `/q/<name>` for a query, `/m/<name>` for a mutation.
    route: String,
    params: &'a [Param],
    /// model the params resolve against (query target / mutation return model);
    /// `None` when it could not be resolved.
    root: Option<&'a RModel>,
    /// The response schema for the `200` body, as a JSON-Schema value.
    response: Value,
    /// The output *object* schema to register in `components.schemas`, deduped by
    /// name across callables (a shape shared by two callables is one schema).
    out_schema: OutSchema,
    /// Whether this is a mutation (drives the summary + `MutationResult` fallback).
    is_mutation: bool,
    /// A `-> stream` query: the `200` body is `application/x-ndjson` (one
    /// row/done/error envelope per line), not a JSON document.
    stream: bool,
    /// How this callable paginates, driving the extra input property:
    /// a keyset page a `cursor` string, an offset page an `offset` integer.
    page: PageInput,
}

/// How a callable paginates, driving its extra request-body property.
/// Mirrors the client emitter's enum so the two request surfaces stay in lockstep.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PageInput {
    None,
    Keyset,
    Offset,
}

/// A named output object schema: a shape projection or a bare-model row.
struct OutSchema {
    name: String,
    /// `(field name, JSON-Schema value, required)` per projected column.
    fields: Vec<(String, Value, bool)>,
    /// A pure update/delete has no declared-shape row to project, so it emits no
    /// object schema — its `200` is the shared `MutationResult`. `true` skips it.
    is_result_fallback: bool,
    /// Named shapes this schema's body references via `field -> Shape` (recursively),
    /// each a full schema of its own — registered in `components.schemas` so the
    /// property `$ref`s resolve, deduped by name across callables.
    nested: Vec<Self>,
}

// ---------- document assembly ----------------------------------------------

fn document(schema: &CheckedSchema, decls: &[Decl]) -> Value {
    let callables = collect(schema, decls);

    // paths: one POST per callable, in declaration order.
    let mut paths = Map::new();
    for c in &callables {
        paths.insert(c.route.clone(), path_item(schema, c));
    }

    // components.schemas: the deduped output object schemas, then the input schemas,
    // plus the two fixed shared schemas (the error envelope + the mutation result).
    let mut schemas = Map::new();
    schemas.insert("Error".to_string(), error_schema());
    schemas.insert("MutationResult".to_string(), mutation_result_schema());
    if schema.mutations.iter().any(|m| m.ack) {
        schemas.insert("Ack".to_string(), ack_schema());
    }

    let mut seen: Vec<String> = Vec::new();
    for c in &callables {
        register_out_schema(&c.out_schema, &mut schemas, &mut seen);
    }
    for c in &callables {
        schemas.insert(input_name(c.name), input_schema(schema, c));
    }

    // `$ctx` rides a header, not the body: model it as a reusable header parameter
    // every operation references; mutations also reference the idempotency-key header.
    let mut parameters = Map::new();
    parameters.insert("BasedContext".to_string(), context_header_param());
    if callables.iter().any(|c| c.is_mutation) {
        parameters.insert("IdempotencyKey".to_string(), idempotency_key_header_param());
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "based API",
            "version": "0.1.0",
            "description": "Generated by `based gen openapi`. The closed RPC surface: \
                            one POST route per query/mutation, carrying \
                            arguments as JSON — never the DSL."
        },
        "paths": Value::Object(paths),
        "components": {
            "parameters": Value::Object(parameters),
            "schemas": Value::Object(schemas),
        }
    })
}

/// Register an output object schema plus every named shape it references (its
/// `nested`), deduped by name — a shape shared by two callables (or referenced from
/// two nests) is one `components.schemas` entry.
fn register_out_schema(os: &OutSchema, schemas: &mut Map<String, Value>, seen: &mut Vec<String>) {
    if !os.is_result_fallback && !seen.contains(&os.name) {
        seen.push(os.name.clone());
        schemas.insert(os.name.clone(), object_schema(&os.fields));
    }
    for n in &os.nested {
        register_out_schema(n, schemas, seen);
    }
}

/// One `paths` entry: a single `post` operation posting the input, returning the
/// response + the shared error responses, and reading `$ctx` from the header. A
/// `-> stream` query's `200` is `application/x-ndjson` — one envelope object per line
/// with a mandatory terminal line; its pre-body failures keep the ordinary JSON
/// error responses.
fn path_item(schema: &CheckedSchema, c: &Callable) -> Value {
    let kind = if c.is_mutation { "Mutation" } else { "Query" };
    let ctx = &callable_ctx(schema, c);
    let ok = if c.stream {
        json!({
            "description": "An NDJSON stream: one `{\"row\":…}` envelope per line, then exactly one \
                            terminal line — `{\"done\":{\"rows\":N}}` on success or \
                            `{\"error\":{code,message}}` on a mid-stream failure. A body that ends \
                            without a terminal line was truncated and must be treated as a \
                            transport error.",
            "content": { "application/x-ndjson": { "schema": c.response.clone() } }
        })
    } else {
        json!({
            "description": "Success.",
            "content": { "application/json": { "schema": c.response.clone() } }
        })
    };
    let mut parameters = vec![json!({ "$ref": "#/components/parameters/BasedContext" })];
    let mut responses = Map::new();
    responses.insert("200".to_string(), ok);
    responses.insert(
        "400".to_string(),
        error_response("Invalid request (bad argument, missing `$ctx`)."),
    );
    responses.insert("404".to_string(), error_response("No such query/mutation."));
    responses.insert(
        "503".to_string(),
        error_response("Retryable database error."),
    );
    if c.is_mutation {
        // A surviving-write mutation whose `where` matches no row is also a 404.
        responses.insert(
            "404".to_string(),
            error_response(
                "No such mutation, or the write matched no row (absent or out of scope).",
            ),
        );
        // Mutations may carry the idempotency key; the 409/422 outcomes exist only
        // for a keyed write.
        parameters.push(json!({ "$ref": "#/components/parameters/IdempotencyKey" }));
        responses.insert(
            "409".to_string(),
            error_response(
                "A request with this idempotency key is still in flight; retry once it settles.",
            ),
        );
        responses.insert(
            "422".to_string(),
            error_response("This idempotency key was already used for a different request."),
        );
        // A guarded mutation can be denied by its host guard before the write runs.
        if let Some(g) = schema
            .mutations
            .iter()
            .find(|m| m.name == c.name)
            .and_then(|m| m.guard.as_deref())
        {
            responses.insert(
                "403".to_string(),
                error_response(&format!("Denied by guard `{g}`.")),
            );
        }
    }
    json!({
        "post": {
            "operationId": c.name,
            "summary": format!("{kind} `{}`", c.name),
            "parameters": Value::Array(parameters),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "$ref": format!("#/components/schemas/{}", input_name(c.name)) }
                    }
                }
            },
            "responses": Value::Object(responses),
            // The `$ctx` fields this callable requires surfaced as a vendor
            // extension — descriptive, not enforced by the wire.
            "x-ctx-requires": Value::Array(ctx.clone()),
        }
    })
}

/// The deduped `$ctx.<field>` requirements as `{ field, type }` objects. A
/// relation-typed field carries the model's key, rendered `-> Model`.
fn callable_ctx(schema: &CheckedSchema, c: &Callable) -> Vec<Value> {
    use based_sema::CtxField;
    // The requirement bag lives on the resolved callable (RQuery/RMutation), keyed by
    // name — the client emitter never needs it, but the spec advertises it.
    let reqs: &[based_sema::CtxReq] = if c.is_mutation {
        schema
            .mutations
            .iter()
            .find(|m| m.name == c.name)
            .map_or(&[][..], |m| m.ctx_requires.as_slice())
    } else {
        schema
            .queries
            .iter()
            .find(|q| q.name == c.name)
            .map_or(&[][..], |q| q.ctx_requires.as_slice())
    };
    reqs.iter()
        .map(|r| {
            let ty = match &r.ty {
                CtxField::Scalar(p) => primitive_name(*p).to_string(),
                CtxField::Relation(m) => format!("-> {m}"),
            };
            json!({ "field": r.field, "type": ty })
        })
        .collect()
}

/// Build the callable descriptors from the checked schema + AST — the OpenAPI twin of
/// the client emitter's `collect`, so the two agree on routes and schema names.
fn collect<'a>(schema: &'a CheckedSchema, decls: &'a [Decl]) -> Vec<Callable<'a>> {
    let queries: std::collections::HashMap<&str, &RQuery> = schema
        .queries
        .iter()
        .map(|q| (q.name.as_str(), q))
        .collect();
    let mutations: std::collections::HashMap<&str, &RMutation> = schema
        .mutations
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();

    let mut out = Vec::new();
    for decl in decls {
        match decl {
            Decl::Query(q) => {
                let Some(rq) = queries.get(q.name.node.as_str()) else {
                    continue;
                };
                let root = schema.model(&rq.target);
                let os = out_schema(schema, decls, &q.ret, root, false);
                out.push(Callable {
                    name: &q.name.node,
                    route: format!("/q/{}", q.name.node),
                    params: &q.params,
                    root,
                    response: query_response(rq, &os, page_with_count(q)),
                    out_schema: os,
                    is_mutation: false,
                    stream: rq.stream,
                    page: page_input(q),
                });
            }
            Decl::Mutation(m) => {
                let Some(rm) = mutations.get(m.name.node.as_str()) else {
                    continue;
                };
                // `-> ok` names no shape/model: the primary written model (sema's
                // `ret_model`) types the params; the `200` is the shared empty `Ack`.
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
                out.push(Callable {
                    name: &m.name.node,
                    route: format!("/m/{}", m.name.node),
                    params: &m.params,
                    root,
                    response,
                    out_schema: os,
                    is_mutation: true,
                    stream: false,
                    page: PageInput::None,
                });
            }
            _ => {}
        }
    }
    out
}

// ---------- responses ------------------------------------------------------

/// A query's `200` schema: stream -> the per-line NDJSON envelope, paginated -> the
/// `Page` envelope, many -> an array, single -> the object (a `get` may miss —
/// modelled as a nullable object).
fn query_response(rq: &RQuery, os: &OutSchema, with_count: bool) -> Value {
    let item = schema_ref(&os.name);
    if rq.stream {
        ndjson_line_schema(&item)
    } else if rq.paginated {
        page_schema(&item, with_count)
    } else if rq.many {
        json!({ "type": "array", "items": item })
    } else {
        // `get`: the row or `null` (keyed lookup may match nothing).
        json!({ "oneOf": [ item, { "type": "null" } ] })
    }
}

/// A `-> stream` query's per-line schema: every NDJSON line is exactly one of the
/// three envelopes — `{"row":…}` per row, the terminal `{"done":{"rows":N}}` on
/// success, or the terminal `{"error":{code,message}}` (the shared error envelope) on
/// a mid-stream failure. A body without a terminal line was truncated.
fn ndjson_line_schema(item: &Value) -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "required": ["row"],
                "properties": { "row": item },
                "description": "One streamed row, in sort order."
            },
            {
                "type": "object",
                "required": ["done"],
                "properties": {
                    "done": {
                        "type": "object",
                        "required": ["rows"],
                        "properties": {
                            "rows": { "type": "integer", "description": "Total rows streamed — an integrity checksum." }
                        }
                    }
                },
                "description": "The terminal success line. A body that ends without `done` or `error` was truncated."
            },
            schema_ref("Error"),
        ]
    })
}

/// A mutation's `200` schema: the declared shape (create-returning), an array if
/// `-> T[]`, or the `{ id }` `MutationResult` fallback (pure update/delete).
fn mutation_response(m: &Mutation, os: &OutSchema) -> Value {
    let item = if os.is_result_fallback {
        schema_ref("MutationResult")
    } else {
        schema_ref(&os.name)
    };
    if m.ret.many {
        json!({ "type": "array", "items": item })
    } else {
        item
    }
}

/// The `Page<T>` envelope schema: rows + an opaque cursor, plus `total` for a
/// `with count` query. Inlined per response so the item type is concrete (no
/// `Page<T>` generic in JSON Schema).
fn page_schema(item: &Value, with_count: bool) -> Value {
    let mut props = json!({
        "rows": { "type": "array", "items": item },
        "cursor": { "type": ["string", "null"], "description": "Opaque keyset cursor; pass it back for the next page." }
    });
    if with_count {
        props["total"] = json!({
            "type": "integer",
            "format": "int64",
            "description": "Total matching rows (the query declares `with count`)."
        });
    }
    json!({
        "type": "object",
        "required": ["rows"],
        "properties": props
    })
}

// ---------- output object schemas ------------------------------------------

/// Resolve a return type to the object schema we register for it. A declared shape
/// projects its body; a bare model (or `full`) projects every stored column. The twin
/// of the client emitter's `out_struct`.
fn out_schema(
    schema: &CheckedSchema,
    decls: &[Decl],
    ret: &RetType,
    root: Option<&RModel>,
    result_fallback: bool,
) -> OutSchema {
    if result_fallback {
        return OutSchema {
            name: "MutationResult".to_string(),
            fields: Vec::new(),
            is_result_fallback: true,
            nested: Vec::new(),
        };
    }
    let name = ret.ty.node.as_str();
    if name != "full" {
        if let Some(shape) = find_shape(decls, name) {
            let model = schema.model(&shape.from.node);
            let mut nested = Vec::new();
            let fields = shape_fields(
                schema,
                decls,
                &shape.body,
                model,
                &mut nested,
                &mut vec![name.to_string()],
            );
            return OutSchema {
                name: name.to_string(),
                fields,
                is_result_fallback: false,
                nested,
            };
        }
    }
    match root {
        Some(m) => OutSchema {
            name: m.name.clone(),
            fields: model_fields(schema, m),
            is_result_fallback: false,
            nested: Vec::new(),
        },
        None => OutSchema {
            name: pascal(name),
            fields: Vec::new(),
            is_result_fallback: false,
            nested: Vec::new(),
        },
    }
}

/// Project a shape body into `(field, schema, required)` triples. A `raw`…`` field
/// maps to the open-object `Json`; a to-**one** nest (`buyer { … }`) becomes an inline
/// nested object schema (recursively projected), required unless the relation is
/// optional. A to-**many** nest (`items { … }`) becomes an `array` of that object
/// schema (always present — an empty array when there are no children). A `field ->
/// Shape` nest `$ref`s the named shape's schema instead of inlining an object; the
/// referenced schema itself lands in `out` (registered once in `components.schemas`).
/// `stack` holds the shape names mid-expansion — the cycle guard (sema rejects
/// reference cycles; this keeps the emitter terminating regardless).
fn shape_fields(
    schema: &CheckedSchema,
    decls: &[Decl],
    body: &[ShapeField],
    model: Option<&RModel>,
    out: &mut Vec<OutSchema>,
    stack: &mut Vec<String>,
) -> Vec<(String, Value, bool)> {
    let mut fields = Vec::new();
    for f in body {
        match f {
            ShapeField::Bare(id) => {
                let (ty, req) = reach_schema(schema, model, &[&id.node]);
                fields.push((id.node.clone(), ty, req));
            }
            ShapeField::Rename { out, value } => match value {
                ShapeValue::Path(p) => {
                    let segs: Vec<&str> = p.segments.iter().map(|s| s.node.as_str()).collect();
                    let (ty, req) = reach_schema(schema, model, &segs);
                    fields.push((out.node.clone(), ty, req));
                }
                ShapeValue::Raw(_) => fields.push((out.node.clone(), json_schema(), false)),
                // An aggregate: `count()` → a required integer; `avg` → a nullable number;
                // `sum`/`min`/`max` → the column's schema, nullable (an empty/all-null
                // group aggregates to null).
                ShapeValue::Agg(agg) => {
                    let (ty, req) = match agg.func.node.as_str() {
                        "count" => (primitive_schema(Primitive::Int), true),
                        "avg" => (primitive_schema(Primitive::Float), false),
                        _ => {
                            let base = agg.arg.as_ref().map_or_else(json_schema, |p| {
                                let segs: Vec<&str> =
                                    p.segments.iter().map(|s| s.node.as_str()).collect();
                                reach_schema(schema, model, &segs).0
                            });
                            (base, false)
                        }
                    };
                    fields.push((out.node.clone(), ty, req));
                }
            },
            ShapeField::Nest { field, body } => {
                if let Some((target, optional)) = to_one_relation(schema, model, &field.node) {
                    let nested = shape_fields(schema, decls, body, Some(target), out, stack);
                    fields.push((field.node.clone(), object_schema(&nested), !optional));
                } else if let Some(target) = to_many_relation(schema, model, &field.node) {
                    // A to-many nest is an array of the element object schema; always
                    // present (empty array when there are no children), so `required`.
                    let nested = shape_fields(schema, decls, body, Some(target), out, stack);
                    let arr = json!({ "type": "array", "items": object_schema(&nested) });
                    fields.push((field.node.clone(), arr, true));
                }
            }
            ShapeField::NestRef { field, shape } => {
                let Some(decl) = find_shape(decls, &shape.node) else {
                    continue;
                };
                if !stack.contains(&shape.node) {
                    stack.push(shape.node.clone());
                    let mut nested = Vec::new();
                    let sfields = shape_fields(
                        schema,
                        decls,
                        &decl.body,
                        schema.model(&decl.from.node),
                        &mut nested,
                        stack,
                    );
                    stack.pop();
                    out.push(OutSchema {
                        name: shape.node.clone(),
                        fields: sfields,
                        is_result_fallback: false,
                        nested,
                    });
                }
                if let Some((_, optional)) = to_one_relation(schema, model, &field.node) {
                    fields.push((field.node.clone(), schema_ref(&shape.node), !optional));
                } else if to_many_relation(schema, model, &field.node).is_some() {
                    let arr = json!({ "type": "array", "items": schema_ref(&shape.node) });
                    fields.push((field.node.clone(), arr, true));
                }
            }
            // `out = edge.far { body }`: the distinct far side, hiding the junction —
            // an array of the far element object schema, always present (empty when
            // there are no far rows).
            ShapeField::Flatten {
                out: alias,
                path,
                body,
            } => {
                if let Some(target) = flatten_far_model(schema, model, path) {
                    let nested = shape_fields(schema, decls, body, Some(target), out, stack);
                    let arr = json!({ "type": "array", "items": object_schema(&nested) });
                    fields.push((alias.node.clone(), arr, true));
                }
            }
        }
    }
    fields
}

/// The far-side model of a flatten path (`edge.far`) — the last segment's relation
/// target. `None` on a malformed path (sema reports it).
fn flatten_far_model<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    path: &Path,
) -> Option<&'a RModel> {
    let mut cur = model?.name.clone();
    let mut out = None;
    for seg in &path.segments {
        let target = match schema.model(&cur)?.member(&seg.node).map(|m| &m.kind)? {
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => target,
            MemberKind::Scalar { .. } => return None,
        };
        out = schema.model(target);
        cur = target.clone();
    }
    out
}

/// The target model + `optional` of a **to-one** relation field, or `None` for a scalar,
/// an unknown field, or a to-**many** edge (a Forward is always to-one; an Inverse is
/// to-one only when its paired forward FK is unique — a one-to-one back edge, which may
/// be absent, hence optional). The OpenAPI twin of the client emitter's `to_one_relation`
/// and the SQL side's `enter_to_one`.
fn to_one_relation<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    field: &str,
) -> Option<(&'a RModel, bool)> {
    match model?.member(field).map(|m| &m.kind)? {
        MemberKind::Forward {
            target, optional, ..
        } => schema.model(target).map(|t| (t, *optional)),
        MemberKind::Inverse { target, via } => {
            let t = schema.model(target)?;
            t.is_unique(via).then_some((t, true))
        }
        MemberKind::Scalar { .. } => None,
    }
}

/// The target model of a to-**many** relation field (an Inverse collection — paired
/// forward FK not unique), or `None` for a scalar / to-one edge. The OpenAPI twin of the
/// SQL side's `to_many_edge`; the field emits as an array of the element object schema.
fn to_many_relation<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    field: &str,
) -> Option<&'a RModel> {
    match model?.member(field).map(|m| &m.kind)? {
        MemberKind::Inverse { target, via } => {
            let t = schema.model(target)?;
            (!t.is_unique(via)).then_some(t)
        }
        _ => None,
    }
}

/// Every stored column of a bare-model return: scalars by their mapped schema, forward
/// FKs as a `uuid` string under the relation field name. Inverse edges store nothing.
fn model_fields(schema: &CheckedSchema, model: &RModel) -> Vec<(String, Value, bool)> {
    let mut fields = Vec::new();
    for mem in &model.members {
        match &mem.kind {
            MemberKind::Scalar {
                enum_name: Some(en),
                optional,
                many,
                ..
            } => fields.push((
                mem.name.clone(),
                wrap(enum_schema(schema, en), *many),
                !*optional,
            )),
            MemberKind::Scalar {
                ty, optional, many, ..
            } => fields.push((
                mem.name.clone(),
                wrap(primitive_schema(*ty), *many),
                !*optional,
            )),
            MemberKind::Forward { optional, .. } => {
                fields.push((mem.name.clone(), uuid_schema(), !*optional));
            }
            MemberKind::Inverse { .. } => {}
        }
    }
    fields
}

// ---------- input object schemas -------------------------------------------

/// The input object schema for a callable: one property per signature param, typed
/// from its explicit annotation or inferred from the column it maps to. A param with a
/// `(default)` or an optional annotation is not `required` (the engine applies the
/// default). `$ctx` is server context (header), never an input.
fn input_schema(schema: &CheckedSchema, c: &Callable) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for p in c.params {
        let optional = p.default.is_some() || p.ty.as_ref().is_some_and(|t| t.optional);
        let ty = param_schema(schema, c.root, p);
        props.insert(p.name.node.clone(), ty);
        if !optional {
            required.push(Value::String(p.name.node.clone()));
        }
    }
    // Page control: a keyset page carries the opaque cursor back, an
    // offset page an explicit offset. Both optional (absent = the first page).
    match c.page {
        PageInput::Keyset => {
            props.insert(
                "cursor".to_string(),
                json!({ "type": ["string", "null"], "description": "Opaque keyset cursor from a prior page's `cursor`; omit for the first page." }),
            );
        }
        PageInput::Offset => {
            props.insert(
                "offset".to_string(),
                json!({ "type": "integer", "minimum": 0, "description": "Row offset; omit for the first page." }),
            );
        }
        PageInput::None => {}
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("object"));
    if !required.is_empty() {
        obj.insert("required".to_string(), Value::Array(required));
    }
    obj.insert("properties".to_string(), Value::Object(props));
    Value::Object(obj)
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

/// A param's JSON-Schema type. Explicit annotation wins — an enum name is that
/// enum's constrained schema, a model type the `uuid` FK string the wire carries —
/// otherwise infer from the bound/same-named column. To-many -> an array.
fn param_schema(schema: &CheckedSchema, root: Option<&RModel>, p: &Param) -> Value {
    match &p.ty {
        Some(te) => {
            if let BaseType::Model(name) = &te.base {
                if schema.enum_(&name.node).is_some() {
                    return wrap(enum_schema(schema, &name.node), te.many);
                }
            }
            wrap(base_schema(&te.base), te.many)
        }
        None => infer_param(schema, root, p),
    }
}

/// Infer an untyped param's schema from how it filters (client emitter's twin): an
/// `-> edge` / same-name relation param is the FK (`uuid`); an `op col` binding / same-
/// name scalar takes that column's schema.
fn infer_param(schema: &CheckedSchema, root: Option<&RModel>, p: &Param) -> Value {
    let field = match &p.binding {
        Some(ParamBinding::Edge(edge)) => &edge.node,
        Some(ParamBinding::ColOp { col, .. }) => &col.node,
        None => &p.name.node,
    };
    reach_schema(schema, root, &[field]).0
}

// ---------- type resolution ------------------------------------------------

/// Resolve a dotted field path against `model` to a `(schema, required)` pair. A scalar
/// terminal is its mapped schema (carrying `many`, and `required = !optional`); a
/// relation terminal is a `uuid` string; intermediate relation hops walk to the target
/// model. Unknown paths (sema already flagged) fall back to the open-object `Json`.
fn reach_schema(schema: &CheckedSchema, model: Option<&RModel>, path: &[&str]) -> (Value, bool) {
    let Some(mut cur) = model else {
        return (uuid_schema(), true);
    };
    let n = path.len();
    for (i, seg) in path.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(seg).map(|m| &m.kind) {
            Some(MemberKind::Scalar {
                enum_name: Some(en),
                optional,
                many,
                ..
            }) => return (wrap(enum_schema(schema, en), *many), !*optional),
            Some(MemberKind::Scalar {
                ty, optional, many, ..
            }) => return (wrap(primitive_schema(*ty), *many), !*optional),
            Some(MemberKind::Forward {
                target, optional, ..
            }) => {
                if last {
                    return (uuid_schema(), !*optional);
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return (json_schema(), false),
                }
            }
            Some(MemberKind::Inverse { target, .. }) => {
                if last {
                    // Terminal to-many reach: an array of ids.
                    return (json!({ "type": "array", "items": uuid_schema() }), true);
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return (json_schema(), false),
                }
            }
            None => return (json_schema(), false),
        }
    }
    (json_schema(), false)
}

/// Wrap a base schema for a to-many field: `many` -> `{ type: array, items: base }`.
fn wrap(base: Value, many: bool) -> Value {
    if many {
        json!({ "type": "array", "items": base })
    } else {
        base
    }
}

/// A primitive's JSON Schema (mirrors the DDL/client mapping). `uuid`/date/
/// timestamp carry the standard OpenAPI `format` so a generator can pick a rich type.
fn primitive_schema(p: Primitive) -> Value {
    match p {
        Primitive::Text => json!({ "type": "string" }),
        Primitive::Int => json!({ "type": "integer", "format": "int64" }),
        Primitive::Bool => json!({ "type": "boolean" }),
        Primitive::Timestamp => json!({ "type": "string", "format": "date-time" }),
        Primitive::Date => json!({ "type": "string", "format": "date" }),
        Primitive::Json => json_schema(),
        Primitive::Uuid | Primitive::Id => uuid_schema(),
        Primitive::Float => json!({ "type": "number", "format": "double" }),
        // A decimal is a string on the wire (lossless), never a JSON float.
        Primitive::Decimal { .. } => json!({ "type": "string", "format": "decimal" }),
    }
}

/// A param/field base schema: a primitive, or a model reference as the `uuid` FK the
/// wire carries.
fn base_schema(b: &BaseType) -> Value {
    match b {
        BaseType::Primitive(p) => primitive_schema(*p),
        BaseType::Model(_) => uuid_schema(),
        // An opaque `raw(…)` value is an unmodelled string on the wire.
        BaseType::Raw(_) => serde_json::json!({ "type": "string" }),
    }
}

/// The `uuid`-string schema (a relation/id FK on the wire).
fn uuid_schema() -> Value {
    json!({ "type": "string", "format": "uuid" })
}

/// An enum column's schema, constrained to the enum's wire values: a string enum is
/// `{ "type": "string", "enum": ["pending", …] }`; an int enum is
/// `{ "type": "integer", "enum": [0, …] }`. Falls back to an open string if the enum is
/// somehow unresolved (sema would have flagged it).
fn enum_schema(schema: &CheckedSchema, name: &str) -> Value {
    use based_sema::{EnumKind, EnumValue};
    let Some(e) = schema.enum_(name) else {
        return json!({ "type": "string" });
    };
    match e.kind {
        EnumKind::Str => {
            let values: Vec<&str> = e
                .variants
                .iter()
                .map(|v| match &v.value {
                    EnumValue::Str(s) => s.as_str(),
                    EnumValue::Int(_) => v.name.as_str(),
                })
                .collect();
            json!({ "type": "string", "enum": values })
        }
        EnumKind::Int => {
            let values: Vec<i64> = e
                .variants
                .iter()
                .map(|v| match &v.value {
                    EnumValue::Int(n) => *n,
                    EnumValue::Str(_) => 0,
                })
                .collect();
            json!({ "type": "integer", "enum": values })
        }
    }
}

/// The open-object `json` schema: any JSON value (a `json` column or a `raw`…`` field).
fn json_schema() -> Value {
    // `true` is JSON Schema for "anything"; OpenAPI 3.1 accepts it.
    Value::Bool(true)
}

/// A primitive's name for the `x-ctx-requires` listing (matches the sema rendering).
fn primitive_name(p: Primitive) -> &'static str {
    match p {
        Primitive::Text => "text",
        Primitive::Int => "int",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "timestamp",
        Primitive::Date => "date",
        Primitive::Json => "json",
        Primitive::Uuid => "uuid",
        Primitive::Id => "Id",
        Primitive::Float => "float",
        Primitive::Decimal { .. } => "decimal",
    }
}

// ---------- fixed shared schemas -------------------------------------------

/// An `object`-typed schema from `(field, schema, required)` triples. An empty body is
/// an open-ended object (a callable with no params posts `{}`).
fn object_schema(fields: &[(String, Value, bool)]) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (f, ty, req) in fields {
        props.insert(f.clone(), ty.clone());
        if *req {
            required.push(Value::String(f.clone()));
        }
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("object"));
    if !required.is_empty() {
        obj.insert("required".to_string(), Value::Array(required));
    }
    obj.insert("properties".to_string(), Value::Object(props));
    Value::Object(obj)
}

/// The error envelope schema: `{ "error": { code, message } }` — exactly what
/// `serve::dispatch` returns for a boundary (`4xx`) or database (`503`) failure.
fn error_schema() -> Value {
    json!({
        "type": "object",
        "required": ["error"],
        "properties": {
            "error": {
                "type": "object",
                "required": ["code", "message"],
                "properties": {
                    "code": { "type": "string" },
                    "message": { "type": "string" }
                }
            }
        }
    })
}

/// The `{ id }` schema a mutation with no declared return shape responds with.
fn mutation_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id"],
        "properties": { "id": uuid_schema() },
        "description": "A write with no declared-shape re-select responds with the id \
                        of the affected row."
    })
}

/// The `-> ok` acknowledgement: an empty object. Registered only when the schema
/// declares an ack mutation.
fn ack_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
        "description": "The empty acknowledgement of a `-> ok` mutation: the delete \
                        ran; a real DELETE leaves no row to return."
    })
}

/// The reusable `$ctx` header parameter: a JSON object an upstream auth proxy sets
/// (`X-Based-Context`). Never a request-body field.
fn context_header_param() -> Value {
    json!({
        "name": "X-Based-Context",
        "in": "header",
        "required": false,
        "description": "Pre-authenticated request context (`$ctx`) as a JSON object, \
                        set by an upstream auth proxy. Carries the \
                        `$ctx.<field>` values a callable requires (see `x-ctx-requires`).",
        "schema": { "type": "string" }
    })
}

/// The reusable mutation idempotency-key header parameter (`Idempotency-Key`): a
/// client-minted opaque key making a retried write run at most once.
fn idempotency_key_header_param() -> Value {
    json!({
        "name": "Idempotency-Key",
        "in": "header",
        "required": false,
        "description": "Optional mutation idempotency key: a retry with the same key \
                        replays the first attempt's response instead of running the \
                        write again. Queries ignore it.",
        "schema": { "type": "string" }
    })
}

// ---------- small helpers --------------------------------------------------

/// A `$ref` to a named component schema.
fn schema_ref(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

/// The standard error-response object referencing the shared `Error` schema.
fn error_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": { "application/json": { "schema": schema_ref("Error") } }
    })
}

fn input_name(name: &str) -> String {
    format!("{}Input", pascal(name))
}

/// snake_case / lower name -> UpperCamel (`order_by_id` -> `OrderById`). Already
/// UpperCamel shape/model names pass through unchanged. (Same rule as the client.)
fn pascal(name: &str) -> String {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut cs = s.chars();
            match cs.next() {
                Some(first) => first.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Find a shape by name in the AST (its body drives the output schema).
fn find_shape<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Shape> {
    decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name => Some(s),
        _ => None,
    })
}
