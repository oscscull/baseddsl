use super::*;

/// Plan a mutation request against a compiled project. Validates the args + `$ctx`
/// (exactly like a query), then generates the engine `id` for every `create` and
/// binds every write statement positionally. The generated ids are seeded into the
/// value environment *before* binding, so a `$name.id` step reference — which lowers to
/// the bound create's `:id_<step>` — resolves to the same value the INSERT used.
/// Assemble a mutation's value environment: each scalar param (skipping a bulk `from`
/// param, whose rows the bulk step reads straight from the request), then the `$ctx`
/// fields. A param's coercion family is the key of the entity it identifies, else its use
/// in the write body.
pub(crate) fn mutation_env(
    compiled: &Compiled,
    ast: &Mutation,
    ctx_requires: &[CtxReq],
    req: &Request,
    from_params: &std::collections::HashSet<&str>,
) -> Result<Env, PlanError> {
    let entities = based_sema::mutation_param_entities(&compiled.schema, ast);
    let mut env = Env::new(compiled.dialect);
    for p in &ast.params {
        if from_params.contains(p.name.node.as_str()) {
            continue;
        }
        let entity = entities.get(&p.name.node).map(String::as_str);
        let (family, optional) = mutation_param_family(compiled, ast, p, entity);
        // An array-typed param used in a write `where (col in $arr)` binds a variable-length
        // list, the same expansion as a query.
        if p.ty.as_ref().is_some_and(|t| t.many) {
            let value = match req.args.get(&p.name.node) {
                Some(v) => coerce_list(v, family).map_err(|e| bad_arg(&p.name.node, e))?,
                None if optional => SqlValue::Null,
                None => return Err(PlanError::MissingArg(p.name.node.clone())),
            };
            env.insert(p.name.node.clone(), value);
            continue;
        }
        env.insert(
            p.name.node.clone(),
            bind_param(&compiled.schema, p, family, optional, req)?,
        );
    }
    for c in ctx_requires {
        bind_ctx_into(&mut env, &compiled.schema, c, req)?;
    }
    Ok(env)
}

pub fn plan_mutation(
    compiled: &Compiled,
    req: &Request,
    id_gen: &dyn IdGen,
) -> Result<MutationPlan, PlanError> {
    let low = compiled
        .mutations
        .get(&req.callable)
        .ok_or_else(|| PlanError::UnknownMutation(req.callable.clone()))?;
    let rm = compiled
        .schema
        .mutations
        .iter()
        .find(|m| m.name == req.callable)
        .ok_or_else(|| PlanError::UnknownMutation(req.callable.clone()))?;
    let ast = find_mutation(&compiled.decls, &req.callable)
        .ok_or_else(|| PlanError::UnknownMutation(req.callable.clone()))?;

    // 1. Assemble the value environment: params, then `$ctx` (no pagination on a
    //    write). A param's family resolves through its use in the write body (the
    //    column it assigns or filters), so the driver can bind it typed.
    // A `create … from $param`'s param carries the row(s) as a shape object/array, not a
    // scalar bind — the bulk step reads it straight from `req.args`. Skip it here so the
    // scalar coercion never sees the array.
    let from_params: std::collections::HashSet<&str> = low
        .stmts
        .iter()
        .filter_map(|w| w.bulk.as_ref().map(|b| b.param.as_str()))
        .collect();
    let mut env = mutation_env(compiled, ast, &rm.ctx_requires, req, &from_params)?;

    // 2. Generate the engine `id` for each create. Record the id of the first create
    //    matching the return model — the row the response identifies. Ids fill uuid
    //    columns, so they bind as the uuid family. A DB-generated (`serial`) create has no
    //    `gen_id` — the DB mints its id, captured at run time.
    let mut result_id = None;
    for w in &low.stmts {
        if let Some(bind) = &w.gen_id {
            let id = match compiled
                .schema
                .model(&w.model)
                .and_then(RModel::pk_strategy)
            {
                Some(based_sema::PkStrategy::Ulid) => id_gen.next_ulid(),
                _ => id_gen.next_id(),
            };
            if result_id.is_none() && w.creates && w.model == rm.ret_model {
                result_id = Some(id.clone());
            }
            env.insert(bind.clone(), SqlValue::Uuid(id));
        }
    }
    // An app-minted return create keys its declared re-select on this known id; a
    // DB-generated one binds `result_id` at run time from its captured row.
    if let Some(id) = &result_id {
        env.insert("result_id".to_string(), SqlValue::Uuid(id.clone()));
    }

    // 3. Build each write step: its unbound `:name` SQL and its row read-back plan (a
    //    bound create's captured columns, coerced by the captured field's family). The run
    //    stage binds each step late, so a `$name.field` reference resolves to the row the
    //    database actually wrote.
    let mut steps = Vec::with_capacity(low.stmts.len());
    for w in &low.stmts {
        let bulk = match &w.bulk {
            Some(bi) => Some(build_bulk_step(compiled, req, id_gen, &env, bi)?),
            None => None,
        };
        steps.push(WriteStep {
            sql: w.sql.clone(),
            capture: w.capture.as_ref().map(|cap| StepCapture {
                cols: cap
                    .cols
                    .iter()
                    .map(|c| CaptureBind {
                        bind: c.bind.clone(),
                        column: c.column.clone(),
                        family: capture_family(&compiled.schema, &w.model, &c.field),
                    })
                    .collect(),
                followup_select: cap.followup_select.clone(),
            }),
            bulk,
        });
    }

    // 4. An `-> ok` mutation's not-found signal is a filtered **real** DELETE (`hard delete
    //    M where …` / a plain-model `delete M where …`) on the primary model affecting zero
    //    rows. A create / update / restore / wipe under `-> ok` (BW1's universal
    //    read-back opt-out) never 404s — a bulk insert of an empty array is a success.
    let ack_check = if rm.ack {
        low.stmts
            .iter()
            .position(|w| w.model == rm.ret_model && w.real_delete)
    } else {
        None
    };

    let bulk_readback = low.bulk_readback.as_ref().map(|br| BulkReadbackPlan {
        sql: br.sql.clone(),
        key_count: br.key_cols.len(),
        bulk: br.bulk,
        serial: br.serial,
    });

    Ok(MutationPlan {
        name: req.callable.clone(),
        dialect: compiled.dialect,
        steps,
        env0: env.snapshot(),
        result_id,
        ret_select: low.ret_select.clone(),
        ack_check,
        bulk_readback,
        json_paths: low.json_paths.clone(),
    })
}
