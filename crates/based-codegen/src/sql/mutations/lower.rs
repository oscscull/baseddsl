use std::collections::HashMap;

use super::*;

/// Lower every mutation in the schema to its structured write statements, in
/// declaration order. The in-process runtime consumes this directly.
pub fn lower_mutations(
    schema: &CheckedSchema,
    decls: &[Decl],
    dialect: Dialect,
) -> Vec<LoweredMutation> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Mutation(m) => Some(lower_mutation(schema, decls, m, dialect)),
            _ => None,
        })
        .collect()
}

fn lower_mutation<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    m: &'a Mutation,
    dialect: Dialect,
) -> LoweredMutation {
    // `unscoped(...)` drops `@scope` from every write in this mutation and the create-time
    // auto-set — the greppable, linted cross-scope escape hatch.
    let unscoped = m.unscoped.is_some();
    // The per-touched-model scope this mutation injects (the chosen alternative), resolved
    // by sema. Empty when `unscoped`. Threaded into every write's `Select`.
    let rm = schema.mutations.iter().find(|rm| rm.name == m.name.node);
    let inject: &[ScopeInject] = rm.map_or(&[][..], |rm| rm.scope_inject.as_slice());
    let ret_model = rm.map_or("", |rm| rm.ret_model.as_str());
    // Which columns each `create … as name` binding's siblings read (`$name.field`), so a
    // bound create's row read-back projects exactly those. Empty outside a `tx`.
    let binding_refs = collect_binding_refs(schema, &m.body);
    let cx = LowerCx {
        schema,
        decls,
        dialect,
        unscoped,
        inject,
        ret_model,
        binding_refs: &binding_refs,
        params: &m.params,
    };
    let mut stmts = Vec::new();
    let no_bindings = HashMap::new();
    // The first `create` of the return model claims the declared re-select's `:result_id`
    // (its read-back captures the id for a DB-generated key); a later same-model create
    // reuses it.
    let mut ret_taken = false;
    for stmt in &m.body {
        lower_write(&cx, stmt, "id", &no_bindings, &mut ret_taken, &mut stmts);
    }

    let ret_select = ret_select(schema, decls, m, rm, &stmts, unscoped, inject, dialect);
    let bulk_readback = bulk_readback(schema, decls, rm, &stmts, unscoped, inject, dialect);

    // A shape-returning mutation reads its written row back through `project_return`, so its
    // `json` leaves need the same structured-JSON normalization a read gets.
    let json_paths = rm
        .and_then(|rm| {
            schema.model(&rm.ret_model).map(|root| {
                json_output_paths(schema, decls, rm.ret_shape.as_deref(), &rm.ret_model, root)
            })
        })
        .unwrap_or_default();

    LoweredMutation {
        name: m.name.node.clone(),
        stmts,
        ret_select,
        bulk_readback,
        json_paths,
    }
}

/// The mutation's writes with any `tx` block flattened inline (execution order), so the
/// re-select search sees the same statement sequence the author wrote.
pub(crate) fn flat_writes(body: &[WriteStmt]) -> Vec<&WriteStmt> {
    let mut out = Vec::new();
    for w in body {
        match w {
            WriteStmt::Tx(inner) => out.extend(inner.iter()),
            other => out.push(other),
        }
    }
    out
}

/// Lower one write statement, pushing its [`LoweredWrite`](s) onto `out`. `id_param` is the
/// bind name a `create`'s app-generated `id` is emitted under (`id` at top level,
/// `id_<step>` inside a `tx` so sibling creates stay distinct); `bindings` is the set of
/// reachable `create … as name` step rows a `$name.field` reads from. `ret_taken` tracks
/// whether the return model's create has already claimed the re-select's `:result_id`. A
/// `tx` flattens: it pushes its inner writes inline and prepends the tx banner to the first.
fn lower_write<'a>(
    cx: &LowerCx<'a>,
    stmt: &'a WriteStmt,
    id_param: &str,
    bindings: &HashMap<&'a str, BackCtx<'a>>,
    ret_taken: &mut bool,
    out: &mut Vec<LoweredWrite>,
) {
    let schema = cx.schema;
    match stmt {
        WriteStmt::Create {
            model,
            assigns,
            from,
            conflict,
            binding,
        } => {
            if let Some(m) = schema.model(&model.node) {
                // A structured shape-input create (`create Model[]? from $param`): the row
                // values come from a shape param, materialized as a chunked multi-row INSERT
                // at run time (sema guarantees eligibility + `-> ok` for the bulk form; a
                // single `from` may still key its declared re-select on the id).
                if let Some(cf) = from {
                    // A from-create of the return model claims the declared read-back (the
                    // bulk IN-keyed re-select, built at the mutation level from this write's
                    // `BulkInsert`); mark it taken so a later same-model write leaves it.
                    if !*ret_taken && m.name == cx.ret_model {
                        *ret_taken = true;
                    }
                    if let Some(w) = lower_bulk_create(cx, m, cf, conflict.as_ref()) {
                        out.push(w);
                    }
                    return;
                }
                // This create's binding read-back columns (its siblings' `$name.field`), and
                // whether it claims the declared re-select's DB-generated `:result_id`.
                let refs = binding
                    .as_ref()
                    .and_then(|b| cx.binding_refs.get(b.node.as_str()))
                    .map_or(&[][..], Vec::as_slice);
                let claims_result = !*ret_taken && m.name == cx.ret_model;
                if claims_result {
                    *ret_taken = true;
                }
                out.push(lower_create(
                    cx,
                    m,
                    assigns,
                    conflict.as_ref(),
                    id_param,
                    bindings,
                    refs,
                    claims_result,
                ));
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_update(cx, m, where_, assigns, bindings));
            }
        }
        WriteStmt::Delete { model, where_ } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_delete(cx, m, where_.as_ref(), false));
            }
        }
        WriteStmt::HardDelete { model, where_ } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_delete(cx, m, where_.as_ref(), true));
            }
        }
        WriteStmt::Restore { model, where_ } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_restore(cx, m, where_));
            }
        }
        WriteStmt::Tx(inner) => lower_tx(cx, inner, bindings, ret_taken, out),
        WriteStmt::Raw(raw) => out.push(raw_write(cx, raw)),
    }
}

/// A raw write: text verbatim, `${param}` -> `:param`. It carries no model, so
/// `{table}`/`{id}` interpolation has no root to bind.
fn raw_write(cx: &LowerCx, raw: &RawSql) -> LoweredWrite {
    LoweredWrite {
        header: String::new(),
        sql: format!("{};\n", render_raw(cx.dialect, raw, "", "")),
        model: String::new(),
        gen_id: None,
        conflict_key: None,
        read_key: None,
        serial_col: None,
        creates: false,
        capture: None,
        wipe: false,
        bulk: None,
        real_delete: false,
    }
}

/// Lower a `tx { … }` block: its inner writes inline in execution order (the engine, not
/// this SQL, owns BEGIN/COMMIT). Sibling `create`s get distinct id binds (`:id_<step>`), and
/// a `create … as name` binding records that step's row so a later `$name.field` (reaching
/// any prior step) resolves against it. A tx banner is prepended to the block's first write
/// (text surface only).
fn lower_tx<'a>(
    cx: &LowerCx<'a>,
    inner: &'a [WriteStmt],
    bindings: &HashMap<&'a str, BackCtx<'a>>,
    ret_taken: &mut bool,
    out: &mut Vec<LoweredWrite>,
) {
    let start = out.len();
    let mut binds = bindings.clone();
    let mut step = 0usize;
    for st in inner {
        let idp = match st {
            WriteStmt::Create { .. } => format!("id_{step}"),
            _ => "id".to_string(),
        };
        lower_write(cx, st, &idp, &binds, ret_taken, out);
        if let WriteStmt::Create { model, binding, .. } = st {
            if let Some(name) = binding {
                binds.insert(
                    name.node.as_str(),
                    BackCtx {
                        model: model.node.as_str(),
                    },
                );
            }
            step += 1;
        }
    }
    if let Some(first) = out.get_mut(start) {
        first.header = format!(
            "-- tx: one engine-owned transaction; rolls back together\n{}",
            first.header
        );
    }
}
