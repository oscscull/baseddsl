use super::*;

/// Plan a query request against a compiled project.
pub fn plan_query(compiled: &Compiled, req: &Request) -> Result<QueryPlan, PlanError> {
    let low = compiled
        .queries
        .get(&req.callable)
        .ok_or_else(|| PlanError::UnknownQuery(req.callable.clone()))?;
    let rq = compiled
        .schema
        .queries
        .iter()
        .find(|q| q.name == req.callable)
        .ok_or_else(|| PlanError::UnknownQuery(req.callable.clone()))?;
    let ast = find_query(&compiled.decls, &req.callable)
        .ok_or_else(|| PlanError::UnknownQuery(req.callable.clone()))?;

    // 1. Assemble the value environment: params, then `$ctx`, then pagination. Each
    //    param binds as its column's family — an untyped param resolves through its
    //    binding against the target model, so a typed (binary-parameter) driver knows
    //    the value's primitive at the bind site.
    let root = compiled.schema.model(&rq.target);
    let entities = based_sema::query_param_entities(&compiled.schema, root, &ast.params);
    let mut env = Env::new(compiled.dialect);
    for p in &ast.params {
        let entity = entities.get(&p.name.node).map(String::as_str);
        let (family, optional) = query_param_family(&compiled.schema, root, p, entity);
        // An array-typed param (`text[]`, `Order[]`, …) binds to a variable-length list for
        // `col in $arr` — each element coerced against the column's scalar `family`, expanded
        // to `(?, ?, …)` at bind time. `family` is already the element (scalar) family.
        let is_list = p.ty.as_ref().is_some_and(|t| t.many);
        if p.optional {
            // A `?` optional filter param: bind a `__present` flag codegen's
            // guard reads, plus the value. Absent → flag 0 (the predicate drops); a value →
            // flag 1 + the value, applied by whatever operator the query used. 2-state —
            // null is no longer a param state (null-matching lives in the body).
            let value = match req.args.get(&p.name.node) {
                Some(v) if is_list => {
                    coerce_list(v, family).map_err(|e| bad_arg(&p.name.node, e))?
                }
                Some(v) => coerce(v, family, true).map_err(|e| bad_arg(&p.name.node, e))?,
                None => SqlValue::Null,
            };
            let present = req.args.contains_key(&p.name.node);
            env.insert(
                format!("{}__present", p.name.node),
                SqlValue::Int(if present { 1 } else { 0 }),
            );
            env.insert(p.name.node.clone(), value);
        } else if is_list {
            let value = match req.args.get(&p.name.node) {
                Some(v) => coerce_list(v, family).map_err(|e| bad_arg(&p.name.node, e))?,
                None => return Err(PlanError::MissingArg(p.name.node.clone())),
            };
            env.insert(p.name.node.clone(), value);
        } else {
            env.insert(
                p.name.node.clone(),
                bind_param(&compiled.schema, p, family, optional, req)?,
            );
        }
    }
    for c in &rq.ctx_requires {
        bind_ctx_into(&mut env, &compiled.schema, c, req)?;
    }
    if offset_paginated(ast) {
        env.insert("offset".to_string(), bind_offset(req)?);
    }
    // Keyset pagination: decode the opaque `cursor` arg into the sort-key values
    // codegen's `:keyset_<i>` placeholders compare against, and flip
    // `:keyset_active`. Absent cursor = the first page: `:keyset_active = 0` short-
    // circuits the comparison to a no-op, and the `:keyset_<i>` bind to NULL (never
    // consulted). `low.keyset` is the codegen-authoritative key list: each cursor
    // value re-binds as its sort column's own primitive.
    if let Some(prims) = &low.keyset {
        bind_cursor(&mut env, req, prims)?;
    }

    // 2. Translate `:name` → `?` for the main and (optional) count statements.
    let main = env.bind(&low.sql)?;
    let count = low.count_sql.as_deref().map(|s| env.bind(s)).transpose()?;

    // 3. The response shape follows the query's inferred cardinality.
    let envelope = match rq.verb {
        Verb::Get => Envelope::One,
        Verb::List if rq.paginated => Envelope::Page {
            with_count: count.is_some(),
        },
        Verb::List => Envelope::Many,
    };

    // A keyset page carries the descriptor the run stage needs to mint the next cursor.
    let keyset = low.keyset.as_ref().map(|prims| KeysetPlan {
        keys: prims.len(),
        page_size: page_size(ast).unwrap_or(u64::MAX),
    });

    Ok(QueryPlan {
        name: req.callable.clone(),
        main,
        count,
        envelope,
        keyset,
        json_paths: low.json_paths.clone(),
    })
}
