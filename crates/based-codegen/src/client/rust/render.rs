use super::*;

pub(crate) fn render(schema: &CheckedSchema, decls: &[Decl], opts: ClientOptions) -> String {
    let callables = collect(schema, decls);
    // The streaming surface (`RowStream`, `decode_ndjson`, `Transport::call_stream`)
    // is emitted only when a `-> stream` query exists, and the idempotency-key
    // surface (`Transport::call_with_key`, the `_with_key` methods) only when a
    // mutation exists — a schema that can't use a surface doesn't carry it, so its
    // module (and the consumer's dependency set) stays exactly as before.
    let has_stream = callables.iter().any(|c| c.stream);
    let has_mutation = callables.iter().any(|c| c.is_mutation);

    let mut out = String::new();
    emit_prelude_transport(&mut out, has_stream, has_mutation);
    emit_entity_tags(&mut out, schema);
    emit_composite_ids(&mut out, schema);
    emit_enums(&mut out, schema);
    let mut seen: Vec<String> = Vec::new();
    emit_output_structs(&mut out, &callables, &mut seen);
    emit_input_shape_structs(&mut out, schema, decls, &callables, &mut seen);
    emit_inputs_and_routes(&mut out, schema, &callables);
    emit_client_impls(&mut out, &callables);

    // Opt-in in-process bridge over `based_runtime::Engine`: a working client with no
    // hand-written `Transport` impl (embedded bridge + the transaction seam). Gated so
    // the wire client stays free of a based-runtime dependency.
    if opts.embedded {
        emit_embedded_bridge(&mut out, &callables, opts, has_stream, has_mutation);
    }
    out
}

/// The fixed prelude + the transport trait, splicing in the streaming/keyed doors the
/// schema actually uses.
fn emit_prelude_transport(out: &mut String, has_stream: bool, has_mutation: bool) {
    out.push_str(PREAMBLE);
    if has_stream {
        out.push_str(STREAMING);
    }
    out.push_str(TRANSPORT_HEAD);
    if has_mutation {
        out.push_str(TRANSPORT_CALL_WITH_KEY);
    }
    if has_stream {
        out.push_str(TRANSPORT_CALL_STREAM);
    }
    out.push_str(TRANSPORT_TAIL);
}

/// One phantom tag per model, so `Id<entity::User>` and `Id<entity::Org>` are distinct
/// types (the tags are types only, never values).
fn emit_entity_tags(out: &mut String, schema: &CheckedSchema) {
    out.push_str("\n/// Phantom entity tags for `Id<entity::M>` (types only, never constructed).\npub mod entity {\n");
    for m in &schema.models {
        out.push_str(&format!("    pub enum {} {{}}\n", m.name));
    }
    out.push_str("}\n");
}

/// One `<M>Id` struct per composite `@key(f1, f2, …)` model, a typed field per key part.
/// The wire form is a JSON object; each part keeps its own phantom typing.
fn emit_composite_ids(out: &mut String, schema: &CheckedSchema) {
    let composite: Vec<&RModel> = schema
        .models
        .iter()
        .filter(|m| m.is_composite_key())
        .collect();
    if !composite.is_empty() {
        out.push_str("\n// ---------- composite ids ----------\n");
        for m in &composite {
            out.push_str(&composite_id_struct(schema, m));
        }
    }
}

/// One real Rust enum per `enum` decl, serde-renamed to the wire variant strings.
fn emit_enums(out: &mut String, schema: &CheckedSchema) {
    if !schema.enums.is_empty() {
        out.push_str("\n// ---------- enums ----------\n");
        for e in &schema.enums {
            out.push_str(&render_enum(e));
        }
    }
}

/// The output structs, deduped by name in first-seen order. An `-> ok` mutation has no
/// output struct; the shared `Ack` decodes its empty body.
fn emit_output_structs(out: &mut String, callables: &[Callable], seen: &mut Vec<String>) {
    out.push_str("\n// ---------- output types ----------\n");
    if callables.iter().any(|c| c.ack) {
        out.push_str(ACK);
    }
    for c in callables {
        if !c.ack {
            emit_struct(out, &c.out_struct, seen);
        }
    }
}

/// A `create … from $param` mutation carries its rows as a shape struct that may appear as
/// no return type, so emit it here, deduped against the output structs.
fn emit_input_shape_structs(
    out: &mut String,
    schema: &CheckedSchema,
    decls: &[Decl],
    callables: &[Callable],
    seen: &mut Vec<String>,
) {
    for c in callables {
        for p in c.params {
            let Some(te) = &p.ty else { continue };
            let BaseType::Model(name) = &te.base else {
                continue;
            };
            if let Some(shape) = find_shape(decls, &name.node) {
                let model = schema.model(&shape.from.node);
                let st = build_struct(
                    schema,
                    decls,
                    name.node.clone(),
                    &shape.body,
                    model,
                    &mut vec![name.node.clone()],
                );
                emit_struct(out, &st, seen);
            }
        }
    }
}

/// The input struct (+ per-callable `Ctx` struct when the callable reads context) and the
/// wire route const, per callable in declaration order.
fn emit_inputs_and_routes(out: &mut String, schema: &CheckedSchema, callables: &[Callable]) {
    out.push_str("\n// ---------- inputs + routes ----------\n");
    for c in callables {
        out.push('\n');
        let fields = input_fields(schema, c);
        out.push_str(&render_input_struct(&input_name(c.name), &fields));
        // A callable that reads `$ctx.<field>`s gets a typed context struct the
        // method takes; a public callable (no requirements) takes `()`.
        if !c.ctx_requires.is_empty() {
            out.push_str(&render_struct(
                &ctx_name(c.name),
                &ctx_fields(schema, c.ctx_requires),
            ));
        }
        out.push_str(&format!(
            "/// Wire route for `{}`.\npub const {}: &str = \"{}\";\n",
            c.name,
            route_const(c.name),
            c.route
        ));
    }
}

/// The client: one typed method per callable posting to its route. A `for update` locking
/// read is held back for the transaction-confined block.
fn emit_client_impls(out: &mut String, callables: &[Callable]) {
    out.push_str("\n// ---------- client ----------\n\n");
    out.push_str("impl<T: Transport> Client<T> {\n");
    for c in callables.iter().filter(|c| !c.for_update) {
        out.push_str(&render_method(c));
    }
    out.push_str("}\n");
    out.push_str(&locking_client_block(callables));
}

/// The in-process bridge over `based_runtime::Engine`, emitted only for an embedded
/// client: the `Embedded` transport (with a working streaming door when the schema has a
/// `-> stream` query), then the transaction seam — the `TxTransport` (engine-owned rungs
/// 1–2) and the `AdoptedTransport` (BYO `adopt`, rung 3), each carrying `TxBound` for
/// `for update` reads and one per-driver `adopt_*` constructor for the compile target.
fn emit_embedded_bridge(
    out: &mut String,
    callables: &[Callable],
    opts: ClientOptions,
    has_stream: bool,
    has_mutation: bool,
) {
    out.push_str(EMBEDDED_HEAD);
    if has_mutation {
        out.push_str(EMBEDDED_KEYED_CALL);
    }
    if has_stream {
        out.push_str(EMBEDDED_STREAM_CALL);
    }
    out.push_str(EMBEDDED_TAIL);
    if has_stream {
        out.push_str(EMBEDDED_ENGINE_ROWS);
    }
    // The read-decide-write transaction seam: the same client over a
    // transaction-bound transport. The keyed/stream doors mirror the trait's optional
    // doors.
    out.push_str(TX_TRANSPORT_HEAD);
    if has_mutation {
        out.push_str(TX_TRANSPORT_KEYED_CALL);
    }
    if has_stream {
        out.push_str(TX_TRANSPORT_STREAM_CALL);
    }
    out.push_str(TX_SURFACE_TAIL);
    // `TxTransport` is the one transport that carries `TxBound`, so the locking reads are
    // callable through it (via `txn.client()` / `transaction`).
    let has_for_update = callables.iter().any(|c| c.for_update);
    if has_for_update {
        out.push_str(TX_BOUND_IMPL);
    }
    // The bring-your-own transaction (`adopt`) rung: the same client over a caller-owned
    // transaction adopted from the caller's own driver. The `AdoptedTransport<D>`
    // `Transport` impl mirrors `TxTransport` (it runs on the held transaction, committing
    // nothing); its `TxBound` impl makes `for update` locking reads work through an
    // adopted client too; and one per-driver `adopt_*` constructor (for the compile-target
    // dialect) hands back a ready `Client`.
    out.push_str(ADOPTED_TRANSPORT_HEAD);
    if has_mutation {
        out.push_str(ADOPTED_TRANSPORT_KEYED_CALL);
    }
    if has_stream {
        out.push_str(ADOPTED_TRANSPORT_STREAM_CALL);
    }
    out.push_str(ADOPTED_TRANSPORT_TAIL);
    if has_for_update {
        out.push_str(ADOPTED_TXBOUND_IMPL);
    }
    if let Some(dialect) = opts.dialect {
        out.push_str(&adopt_constructor(dialect));
    }
}

/// The transaction-confined client block: the `TxBound` marker trait + an
/// `impl<T: Transport + TxBound> Client<T>` carrying the `for update` locking methods, so
/// they are callable only on a transaction-bound transport (a compile error on the
/// auto-commit/wire client). Empty when the schema has no `for update` query — the surface a
/// schema can't use is not emitted.
fn locking_client_block(callables: &[Callable]) -> String {
    if !callables.iter().any(|c| c.for_update) {
        return String::new();
    }
    let mut s = String::from(TXBOUND_TRAIT);
    s.push_str("impl<T: Transport + TxBound> Client<T> {\n");
    for c in callables.iter().filter(|c| c.for_update) {
        s.push_str(&render_method(c));
    }
    s.push_str("}\n");
    s
}
