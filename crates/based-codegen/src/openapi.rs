//! OpenAPI codegen (`based gen openapi`): a `CheckedSchema` -> a single OpenAPI 3.1
//! document describing the JSON/HTTP wire `serve::dispatch` already serves.
//!
//! ## Why an OpenAPI spec, not N per-language emitters (D23)
//! The container door exists to serve non-Rust callers, so the schema must yield
//! clients in many languages. The move is *not* to hand-write a TypeScript emitter,
//! then a Python one, then Go — it is **one** machine-readable contract that
//! `openapi-generator` (or similar) turns into a typed client in any language. So
//! polyglot is one emitter, not N. The **Rust** client stays hand-emitted ([`client`],
//! D22) because it is the in-process `Transport` path (tighter than a generated HTTP
//! stub); everything else falls out of this spec.
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
//! - **`$ctx` is not a body param.** It rides the `X-Based-Context` header (D21,
//!   auth.md/D7), so the spec models it as a header parameter referencing a shared
//!   component, never a request-body field.
//!
//! ## Type mapping (reuses D10/D13, re-projected to JSON Schema)
//! `text`/`uuid`/`timestamp`/`date` -> `string` (`uuid`/`date-time`/`date` formats
//! where they carry one), `int` -> `integer`, `bool` -> `boolean`, `json` -> an open
//! object, a relation FK -> a `uuid` string (the wire carries the id, D1). `optional`
//! -> the property is not `required`; a to-many scalar -> an `array`.
//!
//! ## Deferred (documented, not silently wrong — same gaps as the client)
//! - A to-**one** nested sub-object (`buyer { … }`) is emitted as an inline nested
//!   object schema, matching the client + SQL sides. To-**many** nested arrays are
//!   still skipped (they need JSON aggregation; PLAN L1 follow-up).
//! - A `sql`…`` shape field has no statically known type -> the open-object `Json`.
//! - A **pure** update/delete mutation still responds `{ id }` (its declared-shape
//!   re-select is deferred, D12), so its `200` schema is the `MutationResult` `{ id }`
//!   object, not the declared shape. A create-returning mutation advertises the shape.

use based_ast::*;
use based_sema::{CheckedSchema, MemberKind, RModel, RMutation, RQuery};
use serde_json::{json, Map, Value};

/// Render the whole schema as an OpenAPI 3.1 document (pretty-printed JSON).
pub fn openapi(schema: &CheckedSchema, decls: &[Decl]) -> String {
    let doc = document(schema, decls);
    // Pretty JSON keeps the artifact reviewable (readable > terse, CLAUDE.md); a
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
}

/// A named output object schema: a shape projection or a bare-model row.
struct OutSchema {
    name: String,
    /// `(field name, JSON-Schema value, required)` per projected column.
    fields: Vec<(String, Value, bool)>,
    /// A pure update/delete has no declared-shape row to project, so it emits no
    /// object schema — its `200` is the shared `MutationResult`. `true` skips it.
    is_result_fallback: bool,
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

    let mut seen: Vec<String> = Vec::new();
    for c in &callables {
        if c.out_schema.is_result_fallback || seen.contains(&c.out_schema.name) {
            continue;
        }
        seen.push(c.out_schema.name.clone());
        schemas.insert(
            c.out_schema.name.clone(),
            object_schema(&c.out_schema.fields),
        );
    }
    for c in &callables {
        schemas.insert(input_name(c.name), input_schema(schema, c));
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "based API",
            "version": "0.1.0",
            "description": "Generated by `based gen openapi`. The closed RPC surface \
                            (calling.md): one POST route per query/mutation, carrying \
                            arguments as JSON — never the DSL."
        },
        "paths": Value::Object(paths),
        "components": {
            // `$ctx` rides a header, not the body (D21/D7): model it as a reusable
            // header parameter every operation references.
            "parameters": { "BasedContext": context_header_param() },
            "schemas": Value::Object(schemas),
        }
    })
}

/// One `paths` entry: a single `post` operation posting the input, returning the
/// response + the shared error responses, and reading `$ctx` from the header.
fn path_item(schema: &CheckedSchema, c: &Callable) -> Value {
    let kind = if c.is_mutation { "Mutation" } else { "Query" };
    let ctx = &callable_ctx(schema, c);
    json!({
        "post": {
            "operationId": c.name,
            "summary": format!("{kind} `{}`", c.name),
            "parameters": [ { "$ref": "#/components/parameters/BasedContext" } ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "$ref": format!("#/components/schemas/{}", input_name(c.name)) }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Success.",
                    "content": { "application/json": { "schema": c.response.clone() } }
                },
                "400": error_response("Invalid request (bad argument, missing `$ctx`)."),
                "404": error_response("No such query/mutation."),
                "503": error_response("Retryable database error."),
            },
            // The `$ctx` fields this callable requires (D4/D5) surfaced as a vendor
            // extension — descriptive, not enforced by the wire (auth.md/D7).
            "x-ctx-requires": Value::Array(ctx.clone()),
        }
    })
}

/// The deduped `$ctx.<field>` requirements as `{ field, type }` objects (D4/D5). A
/// relation-typed field carries the model's key (D1), rendered `-> Model`.
fn callable_ctx(schema: &CheckedSchema, c: &Callable) -> Vec<Value> {
    use based_sema::CtxField;
    // The requirement bag lives on the resolved callable (RQuery/RMutation), keyed by
    // name — the client emitter never needs it, but the spec advertises it.
    let reqs: &[based_sema::CtxReq] = if c.is_mutation {
        schema
            .mutations
            .iter()
            .find(|m| m.name == c.name)
            .map(|m| m.ctx_requires.as_slice())
            .unwrap_or(&[])
    } else {
        schema
            .queries
            .iter()
            .find(|q| q.name == c.name)
            .map(|q| q.ctx_requires.as_slice())
            .unwrap_or(&[])
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
                    response: query_response(rq, &os),
                    out_schema: os,
                    is_mutation: false,
                });
            }
            Decl::Mutation(m) => {
                let Some(rm) = mutations.get(m.name.node.as_str()) else {
                    continue;
                };
                let root = schema.model(&m.ret.ty.node).or_else(|| {
                    schema
                        .shapes
                        .iter()
                        .find(|s| s.name == m.ret.ty.node)
                        .and_then(|s| schema.model(&s.from))
                });
                // A mutation only advertises its declared shape when it re-selects one
                // (a create-returning mutation, D12); a pure update/delete responds
                // `{ id }`, so it emits no object schema and points at `MutationResult`.
                let has_reselect = rm.ret_shape.is_some() || schema.model(&m.ret.ty.node).is_some();
                let os = out_schema(schema, decls, &m.ret, root, !has_reselect);
                out.push(Callable {
                    name: &m.name.node,
                    route: format!("/m/{}", m.name.node),
                    params: &m.params,
                    root,
                    response: mutation_response(m, &os),
                    out_schema: os,
                    is_mutation: true,
                });
            }
            _ => {}
        }
    }
    out
}

// ---------- responses ------------------------------------------------------

/// A query's `200` schema: paginated -> the `Page` envelope, many -> an array, single
/// -> the object (a `get` may miss — modelled as a nullable object). (calling.md.)
fn query_response(rq: &RQuery, os: &OutSchema) -> Value {
    let item = schema_ref(&os.name);
    if rq.paginated {
        page_schema(&item)
    } else if rq.many {
        json!({ "type": "array", "items": item })
    } else {
        // `get`: the row or `null` (keyed lookup may match nothing).
        json!({ "oneOf": [ item, { "type": "null" } ] })
    }
}

/// A mutation's `200` schema: the declared shape (create-returning, D12), an array if
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

/// The `Page<T>` envelope schema (calling.md): rows + an opaque cursor, never a bare
/// array. Inlined per response so the item type is concrete (no `Page<T>` generic in
/// JSON Schema).
fn page_schema(item: &Value) -> Value {
    json!({
        "type": "object",
        "required": ["rows"],
        "properties": {
            "rows": { "type": "array", "items": item },
            "cursor": { "type": ["string", "null"], "description": "Opaque keyset cursor; pass it back for the next page." }
        }
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
        };
    }
    let name = ret.ty.node.as_str();
    if name != "full" {
        if let Some(shape) = find_shape(decls, name) {
            let model = schema.model(&shape.from.node);
            return OutSchema {
                name: name.to_string(),
                fields: shape_fields(schema, &shape.body, model),
                is_result_fallback: false,
            };
        }
    }
    match root {
        Some(m) => OutSchema {
            name: m.name.clone(),
            fields: model_fields(m),
            is_result_fallback: false,
        },
        None => OutSchema {
            name: pascal(name),
            fields: Vec::new(),
            is_result_fallback: false,
        },
    }
}

/// Project a shape body into `(field, schema, required)` triples. A `sql`…`` field
/// maps to the open-object `Json`; a to-**one** nest (`buyer { … }`) becomes an inline
/// nested object schema (recursively projected), required unless the relation is
/// optional. A to-**many** nest is skipped (deferred, like the client + SQL sides).
fn shape_fields(
    schema: &CheckedSchema,
    body: &[ShapeField],
    model: Option<&RModel>,
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
            },
            ShapeField::Nest { field, body } => {
                if let Some((target, optional)) = to_one_relation(schema, model, &field.node) {
                    let nested = shape_fields(schema, body, Some(target));
                    fields.push((field.node.clone(), object_schema(&nested), !optional));
                }
                // to-many nest: deferred (L1 array follow-up) — skipped.
            }
        }
    }
    fields
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

/// Every stored column of a bare-model return: scalars by their mapped schema, forward
/// FKs as a `uuid` string under the relation field name. Inverse edges store nothing.
fn model_fields(model: &RModel) -> Vec<(String, Value, bool)> {
    let mut fields = Vec::new();
    for mem in &model.members {
        match &mem.kind {
            MemberKind::Scalar {
                ty, optional, many, ..
            } => fields.push((
                mem.name.clone(),
                wrap(primitive_schema(*ty), *many),
                !*optional,
            )),
            MemberKind::Forward { optional, .. } => {
                fields.push((mem.name.clone(), uuid_schema(), !*optional))
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
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("object"));
    if !required.is_empty() {
        obj.insert("required".to_string(), Value::Array(required));
    }
    obj.insert("properties".to_string(), Value::Object(props));
    Value::Object(obj)
}

/// A param's JSON-Schema type. Explicit annotation wins (a model type -> a `uuid`
/// string, the FK the wire carries, D1); otherwise infer from the bound/same-named
/// column. To-many -> an array.
fn param_schema(schema: &CheckedSchema, root: Option<&RModel>, p: &Param) -> Value {
    match &p.ty {
        Some(te) => wrap(base_schema(&te.base), te.many),
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

/// A primitive's JSON Schema (mirrors the DDL/client mapping, D10/D13). `uuid`/date/
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
    }
}

/// A param/field base schema: a primitive, or a model reference as the `uuid` FK the
/// wire carries (D1).
fn base_schema(b: &BaseType) -> Value {
    match b {
        BaseType::Primitive(p) => primitive_schema(*p),
        BaseType::Model(_) => uuid_schema(),
    }
}

/// The `uuid`-string schema (a relation/id FK on the wire, D1).
fn uuid_schema() -> Value {
    json!({ "type": "string", "format": "uuid" })
}

/// The open-object `json` schema: any JSON value (a `json` column or a `sql`…`` field).
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

/// The `{ id }` schema a pure update/delete mutation responds with (its declared-shape
/// re-select is deferred, D12).
fn mutation_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id"],
        "properties": { "id": uuid_schema() },
        "description": "A write with no declared-shape re-select responds with the id \
                        of the affected row (D12)."
    })
}

/// The reusable `$ctx` header parameter: a JSON object an upstream auth proxy sets
/// (`X-Based-Context`, D21/D7). Never a request-body field.
fn context_header_param() -> Value {
    json!({
        "name": "X-Based-Context",
        "in": "header",
        "required": false,
        "description": "Pre-authenticated request context (`$ctx`) as a JSON object, \
                        set by an upstream auth proxy (auth.md, D7). Carries the \
                        `$ctx.<field>` values a callable requires (see `x-ctx-requires`).",
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
