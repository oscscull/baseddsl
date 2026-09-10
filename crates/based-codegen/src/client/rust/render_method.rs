use super::*;

/// One typed client method: `POST` the input to the route, carry the typed
/// context, decode the output. A callable with `$ctx` requirements takes a
/// `<Name>Ctx`; one with none takes `ctx: ()` (the engine reads no context).
/// A `-> stream` query keeps the same name but goes through the transport's
/// streaming door and hands back the live `RowStream`.
pub(super) fn render_method(c: &Callable) -> String {
    let ctx_ty = if c.ctx_requires.is_empty() {
        "()".to_string()
    } else {
        ctx_name(c.name)
    };
    if c.stream {
        return format!(
                "    /// `POST {route}` — a `-> stream` query: the rows arrive as a live typed\n    /// stream; drop it to cancel the pass.\n    pub async fn {name}(&self, input: {input}, ctx: {ctx_ty}) -> Result<{output}, ClientError> {{\n        self.transport.call_stream({konst}, &input, &ctx).await\n    }}\n",
                route = c.route,
                name = field_ident(c.name),
                input = input_name(c.name),
                ctx_ty = ctx_ty,
                output = c.output,
                konst = route_const(c.name),
            );
    }
    if c.ack {
        // `-> ok`: the wire success is the empty `Ack`; the method returns unit.
        let mut s = format!(
                "    /// `POST {route}` — a `-> ok` mutation: the delete ran (`Ok(())`), or the\n    /// row was absent/out of scope (a `404 not_found` error).\n    pub async fn {name}(&self, input: {input}, ctx: {ctx_ty}) -> Result<(), ClientError> {{\n        let _: Ack = self.transport.call({konst}, &input, &ctx).await?;\n        Ok(())\n    }}\n",
                route = c.route,
                name = field_ident(c.name),
                input = input_name(c.name),
                ctx_ty = ctx_ty,
                konst = route_const(c.name),
            );
        s.push_str(&format!(
                "    /// `POST {route}` carrying `key` as the mutation **idempotency key**: a retry\n    /// with the same key replays the first attempt's response instead of writing again.\n    pub async fn {name}_with_key(\n        &self,\n        input: {input},\n        ctx: {ctx_ty},\n        key: &str,\n    ) -> Result<(), ClientError> {{\n        let _: Ack = self.transport.call_with_key({konst}, &input, &ctx, key).await?;\n        Ok(())\n    }}\n",
                route = c.route,
                // The suffix keeps the name clear of Rust keywords, so no raw-ident escape.
                name = c.name,
                input = input_name(c.name),
                ctx_ty = ctx_ty,
                konst = route_const(c.name),
            ));
        return s;
    }
    let mut s = format!(
            "    /// `POST {route}`\n    pub async fn {name}(&self, input: {input}, ctx: {ctx_ty}) -> Result<{output}, ClientError> {{\n        self.transport.call({konst}, &input, &ctx).await\n    }}\n",
            route = c.route,
            name = field_ident(c.name),
            input = input_name(c.name),
            ctx_ty = ctx_ty,
            output = c.output,
            konst = route_const(c.name),
        );
    if c.is_mutation {
        s.push_str(&format!(
                "    /// `POST {route}` carrying `key` as the mutation **idempotency key**: a retry\n    /// with the same key replays the first attempt's response instead of writing again.\n    pub async fn {name}_with_key(\n        &self,\n        input: {input},\n        ctx: {ctx_ty},\n        key: &str,\n    ) -> Result<{output}, ClientError> {{\n        self.transport.call_with_key({konst}, &input, &ctx, key).await\n    }}\n",
                route = c.route,
                // The suffix keeps the name clear of Rust keywords, so no raw-ident escape.
                name = c.name,
                input = input_name(c.name),
                ctx_ty = ctx_ty,
                output = c.output,
                konst = route_const(c.name),
            ));
    }
    s
}

/// The context fields for a callable: one per required `$ctx.<field>`, typed by
/// the inference (a relation requirement carries the model's key `Uuid`).
pub(super) fn ctx_fields(schema: &CheckedSchema, reqs: &[CtxReq]) -> Vec<(String, String)> {
    reqs.iter()
        .map(|r| {
            let ty = ctx_field_type(schema, &r.ty);
            // An optional `$ctx.field?` read may be absent — the caller passes `None`, and
            // the server present-guards the filter away.
            let ty = if r.optional {
                format!("Option<{ty}>")
            } else {
                ty
            };
            (r.field.clone(), ty)
        })
        .collect()
}

/// A `$ctx` field's Rust type: a scalar by its alias, a relation as that model's
/// typed id (`Id<entity::M>`) — the same mapping the input side uses.
fn ctx_field_type(schema: &CheckedSchema, ty: &CtxField) -> String {
    match ty {
        CtxField::Scalar(p) => primitive(*p).to_string(),
        CtxField::Relation(model) => id_type(schema, model),
    }
}
