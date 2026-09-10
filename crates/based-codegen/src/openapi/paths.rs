//! `paths` entries and their responses: one `post` operation per callable, the query /
//! mutation `200` response schemas, and the reusable `$ctx` / idempotency headers.

use super::*;

/// One `paths` entry: a single `post` operation posting the input, returning the
/// response + the shared error responses, and reading `$ctx` from the header. A
/// `-> stream` query's `200` is `application/x-ndjson` — one envelope object per line
/// with a mandatory terminal line; its pre-body failures keep the ordinary JSON
/// error responses.
pub(crate) fn path_item(schema: &CheckedSchema, c: &Callable) -> Value {
    let kind = if c.is_mutation { "Mutation" } else { "Query" };
    let ctx = callable_ctx(schema, c);
    let mut parameters = vec![json!({ "$ref": "#/components/parameters/BasedContext" })];
    let mut responses = Map::new();
    responses.insert("200".to_string(), ok_response(c));
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
        augment_mutation(schema, c, &mut parameters, &mut responses);
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
            "x-ctx-requires": Value::Array(ctx),
        }
    })
}

/// The `200` body: an NDJSON stream envelope for a `-> stream` query, otherwise the
/// JSON response schema.
fn ok_response(c: &Callable) -> Value {
    if c.stream {
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
    }
}

/// The mutation-only additions to a `post` operation: the row-missing `404`, the
/// idempotency-key header + its `409`/`422` outcomes, and a guard `403`.
fn augment_mutation(
    schema: &CheckedSchema,
    c: &Callable,
    parameters: &mut Vec<Value>,
    responses: &mut Map<String, Value>,
) {
    // A surviving-write mutation whose `where` matches no row is also a 404.
    responses.insert(
        "404".to_string(),
        error_response("No such mutation, or the write matched no row (absent or out of scope)."),
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

/// A query's `200` schema: stream -> the per-line NDJSON envelope, paginated -> the
/// `Page` envelope, many -> an array, single -> the object (a `get` may miss —
/// modelled as a nullable object).
pub(crate) fn query_response(rq: &RQuery, os: &OutSchema, with_count: bool) -> Value {
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
pub(crate) fn mutation_response(m: &Mutation, os: &OutSchema) -> Value {
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

/// The reusable `$ctx` header parameter: a JSON object an upstream auth proxy sets
/// (`X-Based-Context`). Never a request-body field.
pub(crate) fn context_header_param() -> Value {
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
pub(crate) fn idempotency_key_header_param() -> Value {
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

/// The standard error-response object referencing the shared `Error` schema.
fn error_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": { "application/json": { "schema": schema_ref("Error") } }
    })
}
