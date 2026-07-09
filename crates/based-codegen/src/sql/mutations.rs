//! SQL DML generation (write side): a `mutation` body lowers to INSERT /
//! UPDATE / DELETE statements.
//!
//! The headline guarantee here mirrors the read side: a `delete`
//! on a `@soft_delete` model is **rewritten to the tombstone UPDATE — never a real
//! DELETE**. `restore` is its inverse. `hard delete` is the loud, explicit opt-out
//! that does emit a real `DELETE`. The soft-delete live predicate (and `@scope`)
//! is injected into every UPDATE/DELETE `WHERE` so a write can't touch a
//! tombstoned — or out-of-scope — row. The user writes none of this.
//!
//! ## What each action lowers to
//! - `create M { f = $x }` -> `INSERT INTO m (...) VALUES (...)`. The app-generated
//!   `id` (uuid, no SQL default) is bound as `:id` unless the model declares its
//!   own `id`; `@created`/`@updated` columns are set to `CURRENT_TIMESTAMP` (no
//!   DB default).
//! - `update M where (p) { f = $x }` -> `UPDATE m SET ... WHERE p AND <live> [AND
//!   <scope>]`; `@updated` is bumped.
//! - `delete M where (p)` -> soft model: `UPDATE m SET <tombstone> WHERE p AND
//!   <live>`; plain model: `DELETE FROM m WHERE p`.
//! - `restore M where (p)` -> `UPDATE m SET <cleared tombstone> WHERE p` (targets the
//!   deleted rows, so no live predicate is injected).
//! - `hard delete M where (p)` -> real `DELETE FROM m WHERE p` (soft-delete opt-out;
//!   `@scope` still applies).
//! - `tx { ... }` -> the inner statements, run in one engine-owned transaction
//!   (principle 7; the engine, not this SQL, owns BEGIN/COMMIT). Sibling `create`s
//!   get distinct id binds (`:id_<step>`), and a `^.field` back-reference reads the
//!   immediately preceding create — `^.id` binds that create's generated id.
//!
//! ## Returning the declared shape (create-keyed + where-keyed)
//! Every mutation reads its written row back in its declared shape via a trailing
//! re-select (`ret_select`), reusing the read side's `project_return` so the projection
//! can't drift from a `get` (principle 4). The re-select is keyed one of two ways:
//! - **Create-keyed.** A mutation that *creates* its return row keys on the engine id
//!   (`WHERE id = :result_id`, bound by the runtime to that create's generated id).
//! - **Where-keyed.** A mutation whose return row *survives* an `update` / soft `delete` /
//!   `restore` reuses that write's own `where` (its params/`$ctx` are already bound). The
//!   soft-delete live predicate rides along for update/restore (the row is live) but is
//!   dropped for a soft delete (the row is tombstoned — we still read it back).
//!
//! A **real DELETE** (a plain-model `delete` or `hard delete`) has no surviving row to
//! re-select, so it emits none and the response falls back to `{}`.
//!
//! ## Dialects
//! A relation-reaching WHERE lowers to the dialect's multi-table form: MySQL/MariaDB's
//! inline `UPDATE m JOIN ...` / `DELETE m FROM m JOIN ...`, or Postgres's `UPDATE m SET
//! ... FROM j WHERE <on> AND ...` / `DELETE FROM m USING j WHERE <on> AND ...` (Postgres
//! has no inline join in a write, so the join `ON` folds into the WHERE). Postgres
//! also forbids the target alias in `SET`, so a SET column is emitted bare there.

use based_ast::*;
use based_sema::{CheckedSchema, RModel, ScopeInject, SoftDelete, SoftMode};

use crate::sql::dml::{
    physical_col, project_return, push_joins, render_raw, soft_pred, BackCtx, Select,
};
use crate::Dialect;

/// A mutation lowered to its ordered write statements. The whole body already runs
/// under one engine-owned transaction (principle 7), so a `tx { ... }` block is
/// flattened here — its statements sit inline in execution order. The in-process
/// runtime (write path) consumes this directly, exactly as it consumes
/// [`super::LoweredQuery`] for reads, so the executed SQL and its bind surface can
/// never drift from `based gen sql` (principle 4). `render_mutation` (the text
/// emitter) and the runtime both read this one lowering.
#[derive(Debug, Clone)]
pub struct LoweredMutation {
    pub name: String,
    pub stmts: Vec<LoweredWrite>,
    /// The declared-shape re-select: a `SELECT <return shape> FROM <return model> WHERE
    /// <key> [AND <live>] AND <scope>` that reads back the mutation's written row, so the
    /// write response matches the client's decoded output type (the same projection a `get`
    /// of that shape emits, principle 4). `<key>` is either `id = :result_id` for a create
    /// or the write's own `where` for a surviving update / soft delete / restore. `None`
    /// only when the row does not survive the write — a real DELETE (plain-model `delete` /
    /// `hard delete`) — where the response falls back to `{}`.
    pub ret_select: Option<String>,
}

/// One write statement of a mutation: header-free SQL plus the metadata the runtime
/// needs to bind and respond.
#[derive(Debug, Clone)]
pub struct LoweredWrite {
    /// The `-- create …` / `-- delete (soft): …` comment lines the text emitter
    /// frames the SQL with (a `tx` banner is prepended to the block's first write).
    /// The runtime ignores this.
    pub header: String,
    /// Header-free SQL, ending in `;\n`. `:name` placeholders — including the engine
    /// `:id` / `:id_<step>` for a create — are bound by the runtime.
    pub sql: String,
    /// The model this statement writes. A create's model identifies the row the
    /// mutation's declared return refers to (empty for a raw write, which has none).
    pub model: String,
    /// For a `create` whose `id` the engine generates (no caller-set id), the
    /// bind name that id fills (`id`, or `id_<step>` inside a `tx`); else `None`.
    pub gen_id: Option<String>,
}

/// Render every mutation in the schema as its INSERT/UPDATE/DELETE statements, in
/// declaration order, separated by blank lines. Delegates the SQL to
/// [`lower_mutations`] and frames each write with its comment header.
pub fn mutations(schema: &CheckedSchema, decls: &[Decl], dialect: Dialect) -> String {
    // Write SQL branches on the dialect the same way the read side does (identifier
    // quoting, bool/tombstone literals, and — for Postgres — the multi-table
    // UPDATE/DELETE `FROM`/`USING` forms). Only the header names the target here.
    let mut out = String::new();
    out.push_str(&format!(
        "-- Generated by `based gen sql` (dialect: {}). Do not edit by hand.\n",
        dialect.name()
    ));
    out.push_str(
        "-- Mutation templates: `:name` placeholders are bound by the generated client.\n",
    );
    for lm in lower_mutations(schema, decls, dialect) {
        out.push('\n');
        out.push_str(&format!("-- mutation {}\n", lm.name));
        for w in &lm.stmts {
            out.push_str(&w.header);
            out.push_str(&w.sql);
        }
        if let Some(rs) = &lm.ret_select {
            out.push_str("-- return: re-select the written row's declared shape\n");
            out.push_str(rs);
        }
    }
    out
}

/// Lower every mutation in the schema to its structured write statements, in
/// declaration order. The in-process runtime consumes this directly.
pub fn lower_mutations(
    schema: &CheckedSchema,
    decls: &[Decl],
    dialect: Dialect,
) -> Vec<LoweredMutation> {
    let mut out = Vec::new();
    for decl in decls {
        if let Decl::Mutation(m) = decl {
            out.push(lower_mutation(schema, decls, m, dialect));
        }
    }
    out
}

fn lower_mutation<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    m: &'a Mutation,
    dialect: Dialect,
) -> LoweredMutation {
    // `unscoped(...)` drops `@scope` from every write in this mutation *and* the
    // create-time auto-set — the greppable, linted cross-scope escape hatch.
    let unscoped = m.unscoped.is_some();
    // The per-touched-model scope this mutation injects (the chosen alternative),
    // resolved by sema. Empty when `unscoped`. Threaded into every write's `Select`.
    let inject: &[ScopeInject] = schema
        .mutations
        .iter()
        .find(|rm| rm.name == m.name.node)
        .map(|rm| rm.scope_inject.as_slice())
        .unwrap_or(&[]);
    let mut stmts = Vec::new();
    for stmt in &m.body {
        lower_write(
            schema, decls, stmt, "id", None, unscoped, inject, dialect, &mut stmts,
        );
    }
    // Re-select the declared shape whenever the written row survives the mutation. Two
    // key forms (kept identical to the runtime's `plan_mutation`, so codegen and runtime
    // agree on which mutations carry a re-select):
    //   - create-keyed: a write generates the engine `id` of the return model — key on
    //     `:result_id`;
    //   - where-keyed: an `update` / soft `delete` / `restore` on the return model — key
    //     on that write's own `where`.
    // A real DELETE removes the row (no surviving row) → no re-select → `{}` at runtime.
    let ret_select = schema
        .mutations
        .iter()
        .find(|rm| rm.name == m.name.node)
        .and_then(|rm| {
            let creates_ret = stmts
                .iter()
                .any(|w| w.gen_id.is_some() && w.model == rm.ret_model);
            let key = if creates_ret {
                RetKey::CreatedId
            } else {
                let (pred, live) = surviving_ret_write(&m.body, &rm.ret_model, schema)?;
                RetKey::Where { pred, live }
            };
            Some(lower_ret_select(
                schema,
                decls,
                &rm.ret_model,
                rm.ret_shape.as_deref(),
                unscoped,
                inject,
                dialect,
                key,
            ))
        });
    LoweredMutation {
        name: m.name.node.clone(),
        stmts,
        ret_select,
    }
}

/// How a declared-shape re-select keys the row it reads back.
enum RetKey<'a> {
    /// The mutation *created* the row — key on its engine id (`WHERE id = :result_id`,
    /// bound by the runtime). The row is live.
    CreatedId,
    /// The mutation *updated / soft-deleted / restored* the row — key on that write's
    /// own `where` predicate (its params/`$ctx` are already bound). `live` selects whether
    /// the soft-delete live predicate rides along: true for update/restore (the row is
    /// live), false for a soft delete (the row is now tombstoned but must still read back).
    Where { pred: &'a Predicate, live: bool },
}

/// Build the declared-shape re-select for a mutation's written row: the same projection a
/// `get` of that shape emits (`project_return`, reused from the read side so the two can't
/// drift, principle 4), keyed per `key` (created-id or write-`where`). The
/// soft-delete live predicate (when the row is live) and `@scope` ride the read path
/// exactly as a `get` would, so a row that lands / lives out of scope reads back as
/// absent, consistent with every other read.
#[allow(clippy::too_many_arguments)]
fn lower_ret_select(
    schema: &CheckedSchema,
    decls: &[Decl],
    ret_model: &str,
    ret_shape: Option<&str>,
    unscoped: bool,
    inject: &[ScopeInject],
    dialect: Dialect,
    key: RetKey,
) -> String {
    let model = schema
        .model(ret_model)
        .expect("return model resolved by sema");
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_scope_inject(!unscoped)
        .with_scope_terms(inject);

    // Projection first (it seeds joins for reached columns), then the row key + guards
    // (which may seed more joins — a relation-reaching write `where`).
    let projection = project_return(&mut sel, decls, ret_shape, ret_model, model);
    let (mut wheres, live) = match key {
        RetKey::CreatedId => (
            vec![format!("{} = :result_id", sel.qcol(&sel.root_alias, "id"))],
            true,
        ),
        RetKey::Where { pred, live } => (vec![sel.predicate(pred, model)], live),
    };
    if live {
        if let Some(sd) = &model.soft_delete {
            wheres.push(soft_pred(dialect, &sel.root_alias, model, sd));
        }
    }
    if let Some(scope) = sel.scope_where(&sel.root_alias, model) {
        wheres.push(scope);
    }

    let mut sql = format!("SELECT\n{}\nFROM {}", projection, sel.q(&model.table));
    push_joins(&mut sql, dialect, &sel.joins);
    push_where(&mut sql, &wheres);
    sql.push_str(";\n");
    sql
}

/// The write whose surviving row a where-keyed re-select reads back: the first
/// `update` / soft `delete` / `restore` on the return model, with its `where` predicate
/// and whether the row is *live* afterwards (so the re-select injects the soft-delete live
/// predicate). A plain-model / `hard delete` removes the row (no surviving row to read),
/// and a `create` is the create-keyed path — both yield `None` here.
fn surviving_ret_write<'a>(
    body: &'a [WriteStmt],
    ret_model: &str,
    schema: &CheckedSchema,
) -> Option<(&'a Predicate, bool)> {
    for w in flat_writes(body) {
        match w {
            WriteStmt::Update { model, where_, .. } if model.node == ret_model => {
                return Some((where_, true)); // the updated row stays live
            }
            WriteStmt::Restore { model, where_ } if model.node == ret_model => {
                return Some((where_, true)); // the row is live again after a restore
            }
            // A soft `delete` tombstones (the row survives — read it back *without* the
            // live predicate); a plain-model `delete` really removes it (skip — no row).
            WriteStmt::Delete { model, where_ }
                if model.node == ret_model
                    && schema
                        .model(&model.node)
                        .is_some_and(|m| m.soft_delete.is_some()) =>
            {
                return Some((where_, false));
            }
            _ => {}
        }
    }
    None
}

/// The mutation's writes with any `tx` block flattened inline (execution order), so the
/// re-select search sees the same statement sequence the author wrote.
fn flat_writes(body: &[WriteStmt]) -> Vec<&WriteStmt> {
    let mut out = Vec::new();
    for w in body {
        match w {
            WriteStmt::Tx(inner) => out.extend(inner.iter()),
            other => out.push(other),
        }
    }
    out
}

/// Lower one write statement, pushing its [`LoweredWrite`](s) onto `out`. `id_param`
/// is the bind name a `create`'s app-generated `id` is emitted under (`id` at top
/// level, `id_<step>` inside a `tx` so sibling creates stay distinct); `back` is the
/// preceding create a `^.field` reads from. A `tx` flattens: it
/// pushes its inner writes inline and prepends the tx banner to the first of them.
// The lowering context (schema/decls/dialect) + the per-write threading (id_param, back,
// unscoped) genuinely need to ride together; bundling them into a struct would obscure more
// than the arg count costs. The linted trio is intentional.
#[allow(clippy::too_many_arguments)]
fn lower_write<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    stmt: &'a WriteStmt,
    id_param: &str,
    back: Option<BackCtx<'a>>,
    unscoped: bool,
    inject: &'a [ScopeInject],
    dialect: Dialect,
    out: &mut Vec<LoweredWrite>,
) {
    match stmt {
        WriteStmt::Create { model, assigns } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_create(
                    schema, decls, m, assigns, id_param, back, unscoped, inject, dialect,
                ));
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_update(
                    schema, decls, m, where_, assigns, back, unscoped, inject, dialect,
                ));
            }
        }
        WriteStmt::Delete { model, where_ } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_delete(
                    schema, decls, m, where_, false, unscoped, inject, dialect,
                ));
            }
        }
        WriteStmt::HardDelete { model, where_ } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_delete(
                    schema, decls, m, where_, true, unscoped, inject, dialect,
                ));
            }
        }
        WriteStmt::Restore { model, where_ } => {
            if let Some(m) = schema.model(&model.node) {
                out.push(lower_restore(
                    schema, decls, m, where_, unscoped, inject, dialect,
                ));
            }
        }
        WriteStmt::Tx(inner) => {
            let start = out.len();
            // `^` reads the immediately preceding create; number creates so their
            // generated ids get distinct binds and a back-reference can name one.
            let mut prev: Option<BackCtx<'a>> = back;
            let mut step = 0usize;
            for st in inner {
                let idp = match st {
                    WriteStmt::Create { .. } => format!("id_{step}"),
                    _ => "id".to_string(),
                };
                lower_write(
                    schema,
                    decls,
                    st,
                    &idp,
                    prev.clone(),
                    unscoped,
                    inject,
                    dialect,
                    out,
                );
                if let WriteStmt::Create { assigns, .. } = st {
                    prev = Some(BackCtx {
                        id_param: idp,
                        assigns,
                    });
                    step += 1;
                }
            }
            // The tx is one engine-owned transaction (principle 7): the runtime wraps
            // the whole body, so the flattened statements need no per-tx marker beyond
            // this banner on the first write (text surface only).
            if let Some(first) = out.get_mut(start) {
                first.header = format!(
                    "-- tx: one engine-owned transaction (principle 7); rolls back together\n{}",
                    first.header
                );
            }
        }
        // A raw write is an escape hatch: text verbatim, `${param}` -> `:param`.
        // No model is attached, so `{table}`/`{id}` interpolation has no root to bind.
        WriteStmt::Raw(raw) => out.push(LoweredWrite {
            header: String::new(),
            sql: format!("{};\n", render_raw(dialect, raw, "", "")),
            model: String::new(),
            gen_id: None,
        }),
    }
}

// ---------- create ---------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn lower_create<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    model: &RModel,
    assigns: &'a [Assign],
    id_param: &str,
    back: Option<BackCtx<'a>>,
    unscoped: bool,
    inject: &'a [ScopeInject],
    dialect: Dialect,
) -> LoweredWrite {
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_back(back)
        .with_scope_inject(!unscoped)
        .with_scope_terms(inject);
    let mut cols: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    let mut assigned: Vec<String> = Vec::new();

    for a in assigns {
        let col = physical_col(model, &a.col.node);
        cols.push(dialect.quote(&col));
        vals.push(sel.value(&a.value, model));
        assigned.push(col);
    }

    // `@scope` columns are engine-managed on create: auto-set from
    // `:ctx_<field>` for every axis of the alternative this mutation named (sema's
    // per-callable `scope_inject`), so a caller cannot plant a row outside their own
    // scope (cross-scope create is inexpressible; E0186 guarantees a full alternative is
    // named). Sema forbids the caller assigning one (E0181), so on a clean schema
    // `assigned` never contains it — the guard is defensive. Empty when `unscoped`.
    for (field, ctx_field) in sel.scope_terms_for(&model.name).to_vec() {
        let col = physical_col(model, &field);
        if !assigned.contains(&col) {
            cols.push(dialect.quote(&col));
            vals.push(format!(":ctx_{ctx_field}"));
            assigned.push(col);
        }
    }

    // Implicit `id` is app-generated (uuid, no SQL default) — bind it unless the
    // model declares its own `id` that the caller sets explicitly. Only then does the
    // engine generate the id at runtime, under this bind name.
    let gen_id = if !assigned.iter().any(|c| c == "id") {
        cols.insert(0, dialect.quote("id"));
        vals.insert(0, format!(":{id_param}"));
        Some(id_param.to_string())
    } else {
        None
    };

    // `@created`/`@updated` are set on insert, unless the caller already did.
    for col in timestamp_cols(model, &[model.created.as_deref(), model.updated.as_deref()]) {
        if !assigned.contains(&col) {
            cols.push(dialect.quote(&col));
            vals.push("CURRENT_TIMESTAMP".to_string());
        }
    }

    LoweredWrite {
        header: format!("-- create {}\n", model.name),
        sql: format!(
            "INSERT INTO {} ({})\nVALUES ({});\n",
            dialect.quote(&model.table),
            cols.join(", "),
            vals.join(", ")
        ),
        model: model.name.clone(),
        gen_id,
    }
}

// ---------- update ---------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn lower_update<'a>(
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    model: &RModel,
    where_: &Predicate,
    assigns: &'a [Assign],
    back: Option<BackCtx<'a>>,
    unscoped: bool,
    inject: &'a [ScopeInject],
    dialect: Dialect,
) -> LoweredWrite {
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_back(back)
        .with_scope_inject(!unscoped)
        .with_scope_terms(inject);
    let mut sets: Vec<String> = Vec::new();
    let mut assigned: Vec<String> = Vec::new();

    for a in assigns {
        let col = physical_col(model, &a.col.node);
        let val = sel.value(&a.value, model);
        sets.push(format!("{} = {val}", set_lhs(&sel, model, &col)));
        assigned.push(col);
    }
    if let Some(bump) = updated_bump(&sel, model, &assigned) {
        sets.push(bump);
    }

    let mut wheres = vec![sel.predicate(where_, model)];
    inject_guards(&mut sel, model, &mut wheres, /* live = */ true);
    LoweredWrite {
        header: String::new(),
        sql: update_stmt(&sel, model, &sets, &wheres),
        model: model.name.clone(),
        gen_id: None,
    }
}

// ---------- delete / hard delete -------------------------------------------

#[allow(clippy::too_many_arguments)]
fn lower_delete(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: &RModel,
    where_: &Predicate,
    hard: bool,
    unscoped: bool,
    inject: &[ScopeInject],
    dialect: Dialect,
) -> LoweredWrite {
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_scope_inject(!unscoped)
        .with_scope_terms(inject);

    // Soft model + plain `delete` -> tombstone UPDATE, never a real DELETE.
    if let (Some(sd), false) = (&model.soft_delete, hard) {
        let mut sets = vec![tombstone_set(&sel, model, sd, /* deleting = */ true)];
        if let Some(bump) = updated_bump(&sel, model, &[]) {
            sets.push(bump);
        }
        let mut wheres = vec![sel.predicate(where_, model)];
        inject_guards(&mut sel, model, &mut wheres, /* live = */ true);
        return LoweredWrite {
            header: "-- delete (soft): tombstone, never a real DELETE\n".to_string(),
            sql: update_stmt(&sel, model, &sets, &wheres),
            model: model.name.clone(),
            gen_id: None,
        };
    }

    // Plain model, or the loud `hard delete` opt-out -> real DELETE.
    let mut wheres = vec![sel.predicate(where_, model)];
    inject_guards(&mut sel, model, &mut wheres, /* live = */ false);
    let header = if hard {
        "-- hard delete: real DELETE (explicit soft-delete opt-out)\n".to_string()
    } else {
        String::new()
    };
    LoweredWrite {
        header,
        sql: delete_stmt(&sel, model, &wheres),
        model: model.name.clone(),
        gen_id: None,
    }
}

// ---------- restore --------------------------------------------------------

fn lower_restore(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: &RModel,
    where_: &Predicate,
    unscoped: bool,
    inject: &[ScopeInject],
    dialect: Dialect,
) -> LoweredWrite {
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_scope_inject(!unscoped)
        .with_scope_terms(inject);
    // sema (E-restore) guarantees a soft-delete model here; fall back defensively.
    let mut sets = match &model.soft_delete {
        Some(sd) => vec![tombstone_set(&sel, model, sd, /* deleting = */ false)],
        None => Vec::new(),
    };
    if let Some(bump) = updated_bump(&sel, model, &[]) {
        sets.push(bump);
    }
    // Restore targets the *deleted* rows, so the live predicate is NOT injected;
    // `@scope` still applies (you can only restore within your scope) unless `unscoped`.
    let mut wheres = vec![sel.predicate(where_, model)];
    if let Some(scope) = sel.scope_where(&sel.root_alias, model) {
        wheres.push(scope);
    }
    LoweredWrite {
        header: "-- restore: clear the tombstone\n".to_string(),
        sql: update_stmt(&sel, model, &sets, &wheres),
        model: model.name.clone(),
        gen_id: None,
    }
}

// ---------- statement assembly ---------------------------------------------

/// The `SET` clause left-hand side for a column. MySQL/MariaDB accept (and this code
/// emits) a table-qualified `` `t`.`col` ``, which a multi-table UPDATE's `SET` may need to
/// disambiguate the target. **Postgres forbids the target alias in `SET`**, and **SQLite
/// rejects a qualified column in an UPDATE `SET`** (it has no inline-join UPDATE, so the
/// target is always unambiguous) — both take the bare column (`col = …`), the alias
/// belonging only to the `FROM`/`WHERE`. So this qualifies on MySQL/MariaDB and stays bare
/// on Postgres + SQLite.
fn set_lhs(sel: &Select, _model: &RModel, col: &str) -> String {
    match sel.dialect {
        Dialect::Postgres | Dialect::Sqlite => sel.dialect.quote(col),
        _ => sel.qcol(&sel.root_alias, col),
    }
}

/// `UPDATE t [join] SET ... WHERE ...`. A relation-reaching `where` seeds joins, which
/// differ by dialect: MySQL puts them inline (`UPDATE t JOIN j ON … SET …`), Postgres
/// moves the joined tables into a `FROM` list and folds the join `ON` into the `WHERE`
/// (`UPDATE t SET … FROM j WHERE <join-on> AND …`). Without joins both are the plain
/// single-table `UPDATE t SET … WHERE …`.
fn update_stmt(sel: &Select, model: &RModel, sets: &[String], wheres: &[String]) -> String {
    let mut s = format!("UPDATE {}", sel.q(&model.table));
    match sel.dialect {
        Dialect::Postgres => {
            s.push_str(&format!("\nSET {}", sets.join(", ")));
            let mut wheres = wheres.to_vec();
            push_from_using(&mut s, sel, &mut wheres, "FROM");
            push_where(&mut s, &wheres);
        }
        _ => {
            push_joins(&mut s, sel.dialect, &sel.joins);
            s.push_str(&format!("\nSET {}", sets.join(", ")));
            push_where(&mut s, wheres);
        }
    }
    s.push_str(";\n");
    s
}

/// `DELETE FROM t WHERE ...`, or a multi-table delete when the `where` reaches across
/// relations: MySQL's `DELETE t FROM t JOIN …`, Postgres's `DELETE FROM t USING j
/// WHERE <join-on> AND …` (the join tables go in `USING`, the `ON` into `WHERE`).
fn delete_stmt(sel: &Select, model: &RModel, wheres: &[String]) -> String {
    let mut s = String::new();
    match sel.dialect {
        Dialect::Postgres => {
            s.push_str(&format!("DELETE FROM {}", sel.q(&model.table)));
            let mut wheres = wheres.to_vec();
            push_from_using(&mut s, sel, &mut wheres, "USING");
            push_where(&mut s, &wheres);
        }
        _ if sel.joins.is_empty() => {
            s.push_str(&format!("DELETE FROM {}", sel.q(&model.table)));
            push_where(&mut s, wheres);
        }
        _ => {
            s.push_str(&format!(
                "DELETE {} FROM {}",
                sel.q(&sel.root_alias),
                sel.q(&model.table)
            ));
            push_joins(&mut s, sel.dialect, &sel.joins);
            push_where(&mut s, wheres);
        }
    }
    s.push_str(";\n");
    s
}

/// Postgres multi-table form: emit the joined tables as a comma-separated `FROM` (for
/// UPDATE) or `USING` (for DELETE) list, and prepend each join's `ON` condition to the
/// `WHERE` — Postgres has no inline join in an UPDATE/DELETE, so the join predicate
/// becomes an ordinary WHERE conjunct. A `LEFT JOIN`'s outer semantics are lost here,
/// but a mutation `where` only *narrows* the target set (it never projects the joined
/// row), so an inner join is the correct — and only expressible — shape.
fn push_from_using(s: &mut String, sel: &Select, wheres: &mut Vec<String>, keyword: &str) {
    if sel.joins.is_empty() {
        return;
    }
    let tables: Vec<String> = sel
        .joins
        .iter()
        .map(|j| format!("{} AS {}", sel.q(&j.table), sel.q(&j.alias)))
        .collect();
    s.push_str(&format!("\n{keyword} {}", tables.join(", ")));
    // Fold each join `ON` into the WHERE, ahead of the existing conditions.
    let ons: Vec<String> = sel.joins.iter().map(|j| j.on.clone()).collect();
    let mut folded = ons;
    folded.append(wheres);
    *wheres = folded;
}

fn push_where(s: &mut String, wheres: &[String]) {
    if !wheres.is_empty() {
        s.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
    }
}

// ---------- injected guards + engine columns -------------------------------

/// Append the soft-delete live predicate (when `live`) and the callable's chosen
/// `@scope` to a write's `WHERE`, so a mutation can't touch a tombstoned or
/// out-of-scope row. An `unscoped` callable injects no scope (`scope_where` returns
/// `None` — its `scope_inject` is empty); soft-delete still applies (a separate
/// guarantee).
fn inject_guards(sel: &mut Select, model: &RModel, wheres: &mut Vec<String>, live: bool) {
    if live {
        if let Some(sd) = &model.soft_delete {
            wheres.push(soft_pred(sel.dialect, &sel.root_alias, model, sd));
        }
    }
    if let Some(scope) = sel.scope_where(&sel.root_alias, model) {
        wheres.push(scope);
    }
}

/// `@updated` -> `updated_at = CURRENT_TIMESTAMP`, unless the caller set it.
fn updated_bump(sel: &Select, model: &RModel, assigned: &[String]) -> Option<String> {
    let field = model.updated.as_deref()?;
    let col = physical_col(model, field);
    if assigned.contains(&col) {
        return None;
    }
    Some(format!("{} = CURRENT_TIMESTAMP", set_lhs(sel, model, &col)))
}

/// The `SET` fragment that writes (or clears) the tombstone for the covered subset
/// timestamp `CURRENT_TIMESTAMP`/`NULL`, bool `TRUE`/`FALSE`.
fn tombstone_set(sel: &Select, model: &RModel, sd: &SoftDelete, deleting: bool) -> String {
    let col = physical_col(model, &sd.field);
    let val = match (sd.mode, deleting) {
        (SoftMode::Timestamp, true) => "CURRENT_TIMESTAMP".to_string(),
        (SoftMode::Timestamp, false) => "NULL".to_string(),
        (SoftMode::Bool, true) => sel.dialect.bool_lit(true).to_string(),
        (SoftMode::Bool, false) => sel.dialect.bool_lit(false).to_string(),
    };
    format!("{} = {val}", set_lhs(sel, model, &col))
}

/// Resolve the distinct physical columns of the given engine timestamp fields
/// (`@created`/`@updated`), preserving order and dropping `None`s / duplicates.
fn timestamp_cols(model: &RModel, fields: &[Option<&str>]) -> Vec<String> {
    let mut cols: Vec<String> = Vec::new();
    for f in fields.iter().flatten() {
        let col = physical_col(model, f);
        if !cols.contains(&col) {
            cols.push(col);
        }
    }
    cols
}
