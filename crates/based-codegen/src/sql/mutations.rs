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
//!   (the engine, not this SQL, owns BEGIN/COMMIT). Sibling `create`s get distinct id
//!   binds (`:id_<step>`). A bound `create … as name` re-selects its written row (its
//!   INSERT carries `RETURNING <cols>`, or on MySQL a follow-up keyed `SELECT`), and a
//!   later step's `$name.field` reads that row's committed value from a `:bref_name__col`
//!   bind — so a DB default, engine timestamp, or DB-generated id threads across steps.
//!
//! ## Returning the declared shape (create-keyed + where-keyed)
//! Every mutation reads its written row back in its declared shape via a trailing
//! re-select (`ret_select`), reusing the read side's `project_return` so the projection
//! can't drift from a `get`. The re-select is keyed one of two ways:
//! - **Create-keyed.** A mutation that *creates* its return row keys on the engine id
//!   (`WHERE id = :result_id`, bound by the runtime to that create's generated id).
//! - **Where-keyed.** A mutation whose return row *survives* an `update` / soft `delete` /
//!   `restore` reuses that write's own `where` (its params/`$ctx` are already bound). The
//!   soft-delete live predicate rides along for update/restore (the row is live) but is
//!   dropped for a soft delete (the row is tombstoned — we still read it back).
//!
//! A **real DELETE** (a plain-model `delete` or `hard delete`) has no surviving row to
//! re-select, so it emits none and the response falls back to `{}` — the `-> ok`
//! acknowledgement contract (sema rejects a declared shape on a real DELETE).
//!
//! ## Dialects
//! A relation-reaching WHERE lowers to the dialect's multi-table form: MySQL/MariaDB's
//! inline `UPDATE m JOIN ...` / `DELETE m FROM m JOIN ...`, or Postgres's `UPDATE m SET
//! ... FROM j WHERE <on> AND ...` / `DELETE FROM m USING j WHERE <on> AND ...` (Postgres
//! has no inline join in a write, so the join `ON` folds into the WHERE). Postgres
//! also forbids the target alias in `SET`, so a SET column is emitted bare there.

use std::collections::HashMap;

use based_ast::*;
use based_sema::{
    CheckedSchema, MemberKind, PkStrategy, RModel, ScopeInject, SoftDelete, SoftMode,
};

use crate::sql::dml::{
    bref_name, physical_col, project_return, push_joins, render_raw, soft_pred, BackCtx, Select,
};
use crate::Dialect;

/// A mutation lowered to its ordered write statements. The whole body already runs
/// under one engine-owned transaction, so a `tx { ... }` block is
/// flattened here — its statements sit inline in execution order. The in-process
/// runtime (write path) consumes this directly, exactly as it consumes
/// [`super::LoweredQuery`] for reads, so the executed SQL and its bind surface can
/// never drift from `based gen sql`. `render_mutation` (the text
/// emitter) and the runtime both read this one lowering.
#[derive(Debug, Clone)]
pub struct LoweredMutation {
    pub name: String,
    pub stmts: Vec<LoweredWrite>,
    /// The declared-shape re-select: a `SELECT <return shape> FROM <return model> WHERE
    /// <key> [AND <live>] AND <scope>` that reads back the mutation's written row, so the
    /// write response matches the client's decoded output type (the same projection a `get`
    /// of that shape emits). `<key>` is either `id = :result_id` for a create
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
    /// For an upsert `create … on conflict (…)`, the read-back key: each conflict
    /// column's `(physical_col, value_sql)` — the value the create sets for it. The
    /// declared-shape re-select keys on this (not the INSERT's generated id, which a
    /// conflict path discards), so the winning row reads back on both paths. `None` for
    /// a plain create / any other write.
    pub conflict_key: Option<Vec<(String, String)>>,
    /// For a `create` on a **keyless** (`@no_id`) model, the read-back key: a `(unique)`
    /// column the create sets, as `(physical_col, value_sql)` — the declared-shape
    /// re-select keys on it since there is no generated `id`. `None` for a keyed model /
    /// any other write.
    pub read_key: Option<Vec<(String, String)>>,
    /// The physical column whose DB-generated value the run stage recovers for a `serial`
    /// (DB-generated PK) create: the sole `serial` `id`, or a composite `@key`'s `serial`
    /// part. `None` for an app-minted / keyless / natural-key create. Its value is
    /// captured by [`capture`](Self::capture) (a `result_id` bind); `serial_col` marks the
    /// column so the re-select keying knows the id is DB-generated.
    pub serial_col: Option<String>,
    /// Whether this statement is a `create` — the mutation's declared re-select keys on a
    /// create of the return model (`:result_id`), so the assembler needs to know which
    /// writes create.
    pub creates: bool,
    /// For a bound `create` (`create … as name`) and/or a `serial`/composite return
    /// create, the row read-back: after the INSERT runs, the run stage captures the
    /// listed columns' committed values into per-column binds a later step (or the
    /// declared re-select) reads. `None` for a create that neither binds a step nor needs
    /// a DB-generated id read back, and for every non-create write.
    pub capture: Option<Capture>,
    /// A whole-table wipe (`delete all` / `hard delete all`): no `where` narrows it, so
    /// "zero rows affected" is a legitimate success (the table was already empty), not the
    /// absent-row 404 an ordinary delete's zero rows signals. The runtime therefore skips
    /// the ack-row-count check for a wipe.
    pub wipe: bool,
    /// A structured shape-input create (`create Model from $row` / `create Model[] from
    /// $rows`, BW1): the row values come from a shape-typed param, not the `:name`
    /// template in `sql`. The runtime reads the param, expands it to a chunked, atomic
    /// multi-row `INSERT`. `None` for every ordinary inline write. When `Some`, `sql`
    /// carries only a review-only single-row template.
    pub bulk: Option<BulkInsert>,
    /// A filtered **real** DELETE (`hard delete M where …`, or a plain-model `delete M
    /// where …`) — the only write whose zero-rows-affected is an absent-row 404 under an
    /// `-> ok` acknowledgement (D98). A wipe, a soft tombstone, a create, and an update
    /// all leave it `false`, so a surviving-write `-> ok` (BW1's relaxed E0221) never 404s.
    pub real_delete: bool,
}

/// A structured shape-input `create` (BW1). The runtime materializes the actual SQL — the
/// row count is dynamic, so codegen carries the column plan rather than a finished
/// statement. Rows above the driver's bind limit are transparently chunked; the whole
/// insert is one atomic unit within the surrounding transaction.
#[derive(Debug, Clone)]
pub struct BulkInsert {
    /// The target model (the runtime resolves per-column coercion families + the id
    /// strategy from it).
    pub model: String,
    /// The fully-qualified, quoted table name.
    pub table: String,
    /// The mutation param the row(s) come from — a JSON object (single) or array (bulk).
    pub param: String,
    /// `Model[] from` (many rows) vs `Model from` (one row).
    pub bulk: bool,
    /// The INSERT columns in order, each with the per-row value source.
    pub columns: Vec<BulkCol>,
    /// A DB-generated `serial` id column to `RETURNING` after insert, so a single
    /// `create Model from $row -> Shape` can key its declared re-select on it. Empty for
    /// an app-minted / natural key, a bulk (`-> ok`) insert, or a keyless model.
    pub returning: Vec<String>,
}

/// One INSERT column of a structured shape-input create: the physical column and where its
/// per-row value comes from.
#[derive(Debug, Clone)]
pub struct BulkCol {
    /// Physical column name (unquoted — the runtime quotes per dialect).
    pub column: String,
    pub source: BulkSource,
}

/// The per-row value source for one bulk-insert column (BW1). The presence-driven rule:
/// a column named in the shape is written verbatim from the payload; an absent
/// engine-managed column is filled by the engine; `@scope` is *always* engine-injected.
#[derive(Debug, Clone)]
pub enum BulkSource {
    /// `row[json_key]` — a scalar column written verbatim from the payload. `field` names
    /// the model member whose type coerces the value (usually == `json_key`).
    Field {
        json_key: String,
        field: String,
    },
    /// `row[relation][key_field]` — an FK column linking an existing row (a nested
    /// `rel { key }` block in the input shape).
    FkPart {
        relation: String,
        key_field: String,
    },
    /// An app-minted id per row (`uuid` / `ulid`), absent from the shape.
    MintUuid,
    MintUlid,
    /// A `@scope` column — always the caller's `$ctx.<field>`, identical for every row
    /// (never taken from the payload, even when the shape names it).
    Ctx {
        ctx_field: String,
    },
    /// `CURRENT_TIMESTAMP` — an engine `@created`/`@updated` stamp, absent from the shape.
    Now,
}

/// A bound `create`'s row read-back: the committed column values the run stage captures
/// after the INSERT, so a later `tx` step's `$name.field` (and a DB-generated id's
/// `:result_id`) reads the row the database actually wrote.
#[derive(Debug, Clone)]
pub struct Capture {
    /// Each column to capture: the bind a later step reads it under, the physical column
    /// it comes from, and the field name (a member of the created model) whose type the
    /// run stage coerces the value by.
    pub cols: Vec<CaptureCol>,
    /// On Postgres/SQLite/MariaDB the INSERT's own `RETURNING <cols>` returns the row, so
    /// this is `None`. On MySQL (no `INSERT … RETURNING`) it is a follow-up keyed `SELECT`
    /// (unbound `:name` SQL) the run stage executes right after the INSERT to read the row.
    pub followup_select: Option<String>,
}

/// One captured column of a bound create's re-selected row.
#[derive(Debug, Clone)]
pub struct CaptureCol {
    /// The `:name` bind (without the colon) a later step / the re-select reads this value
    /// under — `bref_<binding>__<column>`, or `result_id` for a DB-generated return id.
    pub bind: String,
    /// The physical column read back from the written row.
    pub column: String,
    /// The created model's field whose type coerces the captured value at the later bind.
    pub field: String,
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
            // MySQL reads a bound create's written row back with a follow-up keyed SELECT
            // (no `INSERT … RETURNING`); show it for review parity with the RETURNING form.
            if let Some(sel) = w.capture.as_ref().and_then(|c| c.followup_select.as_ref()) {
                out.push_str("-- read-back: the written row's captured columns\n");
                out.push_str(sel);
            }
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
    // `unscoped(...)` drops `@scope` from every write in this mutation *and* the
    // create-time auto-set — the greppable, linted cross-scope escape hatch.
    let unscoped = m.unscoped.is_some();
    // The per-touched-model scope this mutation injects (the chosen alternative),
    // resolved by sema. Empty when `unscoped`. Threaded into every write's `Select`.
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
    // does not re-claim it.
    let mut ret_taken = false;
    for stmt in &m.body {
        lower_write(&cx, stmt, "id", &no_bindings, &mut ret_taken, &mut stmts);
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
            // An upsert (`create … on conflict`) on the return model keys on the conflict
            // target (a conflict path keeps the existing row's id, so the generated id
            // won't match); a plain create keys on that id; an update / soft delete /
            // restore keys on its own `where`.
            let upsert = stmts
                .iter()
                .find(|w| w.conflict_key.is_some() && w.model == rm.ret_model);
            // A composite `@key` create with a DB-generated `serial` part keys the re-select
            // on the captured serial value (`:result_id`) *and* its other app-supplied key
            // parts. (Its `serial_col` is set and `read_key` carries the other parts.)
            let composite_serial = stmts.iter().find(|w| {
                w.serial_col.is_some() && w.read_key.is_some() && w.model == rm.ret_model
            });
            // A keyless create reads back by the `(unique)` column it set, not a
            // generated id — the same `WHERE col = value` shape as a conflict key.
            let keyless = stmts.iter().find(|w| {
                w.read_key.is_some() && w.serial_col.is_none() && w.model == rm.ret_model
            });
            // A create of the return row keys the re-select on that row's id — whether the
            // id is app-minted (`gen_id`) or DB-generated (a sole `serial` id, bound late by
            // the runtime from the captured id). Both use `WHERE id = :result_id`.
            let creates_ret = stmts
                .iter()
                .any(|w| (w.gen_id.is_some() || w.serial_col.is_some()) && w.model == rm.ret_model);
            let key = if let Some(w) = upsert {
                RetKey::Conflict(w.conflict_key.clone().unwrap_or_default())
            } else if let Some(w) = composite_serial {
                RetKey::CompositeSerial {
                    serial_col: w.serial_col.clone().unwrap_or_default(),
                    others: w.read_key.clone().unwrap_or_default(),
                }
            } else if let Some(w) = keyless {
                RetKey::Conflict(w.read_key.clone().unwrap_or_default())
            } else if creates_ret {
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
    /// The mutation *upserted* the row (`create … on conflict`) — key on the conflict
    /// target's inserted value, since a conflict path keeps the existing row's id (so the
    /// INSERT's generated id won't match it). Each pair is `(physical_col, value_sql)`. The
    /// row is live (upsert is disallowed on a soft-delete model).
    Conflict(Vec<(String, String)>),
    /// The mutation created a composite-`@key` row with a DB-generated `serial` part — key
    /// on the captured serial value (`serial_col = :result_id`, bound late by the runtime)
    /// AND the other app-supplied key parts (`others`, each `(physical_col, value_sql)`).
    CompositeSerial {
        serial_col: String,
        others: Vec<(String, String)>,
    },
}

/// Build the declared-shape re-select for a mutation's written row: the same projection a
/// `get` of that shape emits (`project_return`, reused from the read side so the two can't
/// drift), keyed per `key` (created-id or write-`where`). The
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
        RetKey::Conflict(pairs) => (
            pairs
                .iter()
                .map(|(c, v)| format!("{} = {v}", sel.qcol(&sel.root_alias, c)))
                .collect(),
            true,
        ),
        RetKey::CompositeSerial { serial_col, others } => {
            let mut wheres = vec![format!(
                "{} = :result_id",
                sel.qcol(&sel.root_alias, &serial_col)
            )];
            wheres.extend(
                others
                    .iter()
                    .map(|(c, v)| format!("{} = {v}", sel.qcol(&sel.root_alias, c))),
            );
            (wheres, true)
        }
    };
    if live {
        if let Some(sd) = &model.soft_delete {
            wheres.push(soft_pred(dialect, &sel.root_alias, model, sd));
        }
    }
    if let Some(scope) = sel.scope_where(&sel.root_alias, model) {
        wheres.push(scope);
    }

    let mut sql = format!("SELECT\n{}\nFROM {}", projection, sel.qt(model));
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
            // A soft `delete all` (`where_` = `None`) tombstones every row — no single
            // surviving row to read back, so it is an `-> ok` ack (skip here too).
            WriteStmt::Delete {
                model,
                where_: Some(where_),
            } if model.node == ret_model
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

/// Which columns each `create … as name` binding's siblings read (`$name.field`), so the
/// bound create's row read-back projects exactly those. Walks the flattened body: first
/// maps each binding to its model, then collects every `$name.field` reference (an assign
/// RHS, an arithmetic operand, a composite-FK whole-row `$name`, or a `where` comparison)
/// to a captured column. Mirrors `Select::binding_field_value`'s resolution so the
/// captured columns match the emitted `:bref_<name>__<column>` binds.
fn collect_binding_refs<'a>(
    schema: &'a CheckedSchema,
    body: &'a [WriteStmt],
) -> HashMap<String, Vec<CaptureCol>> {
    let flat = flat_writes(body);
    let mut models: HashMap<&str, &RModel> = HashMap::new();
    for w in &flat {
        if let WriteStmt::Create {
            model,
            binding: Some(b),
            ..
        } = w
        {
            if let Some(m) = schema.model(&model.node) {
                models.insert(b.node.as_str(), m);
            }
        }
    }
    let mut out: HashMap<String, Vec<CaptureCol>> = HashMap::new();
    for w in &flat {
        match w {
            WriteStmt::Create {
                model,
                assigns,
                conflict,
                ..
            } => {
                let m = schema.model(&model.node);
                collect_assign_refs(schema, m, assigns, &models, &mut out);
                if let Some(oc) = conflict {
                    collect_assign_refs(schema, m, &oc.update, &models, &mut out);
                }
            }
            WriteStmt::Update {
                model,
                where_,
                assigns,
            } => {
                collect_assign_refs(
                    schema,
                    schema.model(&model.node),
                    assigns,
                    &models,
                    &mut out,
                );
                collect_pred_refs(where_, &models, &mut out);
            }
            WriteStmt::Restore { where_, .. } => collect_pred_refs(where_, &models, &mut out),
            WriteStmt::Delete { where_, .. } | WriteStmt::HardDelete { where_, .. } => {
                if let Some(p) = where_ {
                    collect_pred_refs(p, &models, &mut out);
                }
            }
            WriteStmt::Tx(_) | WriteStmt::Raw(_) => {}
        }
    }
    out
}

/// Record `$binding.field` as a captured column of the bound model: its physical column,
/// deduped, under the `bref_<binding>__<column>` bind a `$binding.field` reference reads.
fn add_ref(
    out: &mut HashMap<String, Vec<CaptureCol>>,
    models: &HashMap<&str, &RModel>,
    binding: &str,
    field: &str,
) {
    let Some(m) = models.get(binding) else { return };
    let column = physical_col(m, field);
    let cols = out.entry(binding.to_string()).or_default();
    if !cols.iter().any(|c| c.column == column) {
        cols.push(CaptureCol {
            bind: bref_name(binding, &column),
            column,
            field: field.to_string(),
        });
    }
}

/// Collect binding references from a write's assign block.
fn collect_assign_refs(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    assigns: &[Assign],
    models: &HashMap<&str, &RModel>,
    out: &mut HashMap<String, Vec<CaptureCol>>,
) {
    for a in assigns {
        collect_rhs_refs(schema, model, &a.col.node, &a.value, models, out);
    }
}

/// Collect binding references from one assign RHS. A whole-row `$name` filling a
/// composite-FK column references every key part of the bound model; a `$name.field`
/// references that one field; an arithmetic RHS recurses into its operands.
fn collect_rhs_refs(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    col: &str,
    rhs: &AssignRhs,
    models: &HashMap<&str, &RModel>,
    out: &mut HashMap<String, Vec<CaptureCol>>,
) {
    match rhs {
        AssignRhs::Value(Value::Param(pr)) if models.contains_key(pr.name.node.as_str()) => {
            let binding = pr.name.node.as_str();
            if let Some(field) = pr.path.first() {
                add_ref(out, models, binding, &field.node);
            } else if let Some(mem) = model.and_then(|m| m.member(col)) {
                // Whole-row `$name` into a composite-FK column: capture every key part.
                if matches!(mem.kind, MemberKind::Forward { .. }) {
                    let parts = schema.fk_columns(mem);
                    if parts.len() > 1 {
                        for (_, part) in parts {
                            add_ref(out, models, binding, &part.name);
                        }
                    }
                }
            }
        }
        AssignRhs::Value(_) => {}
        AssignRhs::Arith { lhs, rhs, .. } => {
            collect_rhs_refs(schema, model, col, lhs, models, out);
            collect_rhs_refs(schema, model, col, rhs, models, out);
        }
    }
}

/// Collect binding references from a `where` predicate: a `$name.field` on either side of a
/// comparison, in an `in (…)` list, or passed as a named-filter argument.
fn collect_pred_refs(
    pred: &Predicate,
    models: &HashMap<&str, &RModel>,
    out: &mut HashMap<String, Vec<CaptureCol>>,
) {
    let record = |out: &mut HashMap<String, Vec<CaptureCol>>, v: &Value| {
        if let Value::Param(pr) = v {
            if let Some(field) = pr.path.first() {
                add_ref(out, models, &pr.name.node, &field.node);
            }
        }
    };
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            collect_pred_refs(a, models, out);
            collect_pred_refs(b, models, out);
        }
        Predicate::Not(inner) => collect_pred_refs(inner, models, out),
        Predicate::Cmp { value, .. } => record(out, value),
        Predicate::InList { values, .. } => values.iter().for_each(|v| record(out, v)),
        Predicate::FilterCall { args, .. } => args.iter().for_each(|v| record(out, v)),
        Predicate::Bare(_) | Predicate::Raw(_) => {}
    }
}

/// The immutable lowering context for a mutation body: schema/decls/dialect, the
/// `unscoped` + chosen-scope decisions, the return model, and the precomputed per-binding
/// read-back columns. Threaded through every write of the body.
struct LowerCx<'a> {
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    dialect: Dialect,
    unscoped: bool,
    inject: &'a [ScopeInject],
    ret_model: &'a str,
    binding_refs: &'a HashMap<String, Vec<CaptureCol>>,
    /// The mutation's params — a `create … from $param`'s input shape is resolved through
    /// the param's declared type.
    params: &'a [Param],
}

/// Lower one write statement, pushing its [`LoweredWrite`](s) onto `out`. `id_param`
/// is the bind name a `create`'s app-generated `id` is emitted under (`id` at top
/// level, `id_<step>` inside a `tx` so sibling creates stay distinct); `bindings` is the
/// set of reachable `create … as name` step rows a `$name.field` reads from. `ret_taken`
/// tracks whether the return model's create has already claimed the re-select's
/// `:result_id`. A `tx` flattens: it pushes its inner writes inline and prepends the tx
/// banner to the first.
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
                // values come from a shape param, materialized as a chunked multi-row
                // INSERT at run time (sema guarantees eligibility + `-> ok` for the bulk
                // form; a single `from` may still key its declared re-select on the id).
                if let Some(cf) = from {
                    let claims_result = !*ret_taken && m.name == cx.ret_model;
                    if claims_result {
                        *ret_taken = true;
                    }
                    if let Some(w) = lower_bulk_create(cx, m, cf, claims_result) {
                        out.push(w);
                    }
                    return;
                }
                // This create's binding read-back columns (its siblings' `$name.field`),
                // and whether it claims the declared re-select's DB-generated `:result_id`.
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
        // A raw write is an escape hatch: text verbatim, `${param}` -> `:param`.
        // No model is attached, so `{table}`/`{id}` interpolation has no root to bind.
        WriteStmt::Raw(raw) => out.push(LoweredWrite {
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
        }),
    }
}

/// Lower a `tx { … }` block: its inner writes inline in execution order (the engine, not
/// this SQL, owns BEGIN/COMMIT). Sibling `create`s get distinct id binds (`:id_<step>`),
/// and a `create … as name` binding records that step's row so a later `$name.field`
/// (reaching any prior step) resolves against it. A tx banner is prepended to the block's
/// first write (text surface only).
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
            "-- tx: one engine-owned transaction (principle 7); rolls back together\n{}",
            first.header
        );
    }
}

// ---------- create ---------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn lower_create<'a>(
    cx: &LowerCx<'a>,
    model: &RModel,
    assigns: &'a [Assign],
    conflict: Option<&'a OnConflict>,
    id_param: &str,
    bindings: &HashMap<&'a str, BackCtx<'a>>,
    refs: &[CaptureCol],
    claims_result: bool,
) -> LoweredWrite {
    let (schema, decls, dialect) = (cx.schema, cx.decls, cx.dialect);
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_bindings(bindings.clone())
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);
    let mut cols: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    let mut assigned: Vec<String> = Vec::new();
    // field name → the value SQL the INSERT sets for it, for building the conflict key.
    let mut value_by_field: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for a in assigns {
        // A relation into a composite-key model is a multi-column FK: the one assign
        // (`enrollment = $e`) fills every `<field>_<part>` column, each part's value pulled
        // from the RHS (a tx binding's key-part assign, or a structured-id param's part).
        let fk_cols = model
            .member(&a.col.node)
            .filter(|m| matches!(m.kind, MemberKind::Forward { .. }))
            .map(|m| schema.fk_columns(m))
            .filter(|p| p.len() > 1);
        if let Some(pairs) = fk_cols {
            for (fk_col, part) in &pairs {
                let val = sel.fk_assign_part(&a.value, &part.name);
                cols.push(dialect.quote(fk_col));
                vals.push(val);
                assigned.push(fk_col.clone());
            }
            continue;
        }
        let col = physical_col(model, &a.col.node);
        cols.push(dialect.quote(&col));
        // An enum column takes a bare variant → its wire string literal.
        let val = sel.assign_rhs(&a.value, model, &a.col.node);
        value_by_field.insert(a.col.node.clone(), val.clone());
        vals.push(val);
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
        value_by_field
            .entry(field.clone())
            .or_insert_with(|| format!(":ctx_{ctx_field}"));
        if !assigned.contains(&col) {
            cols.push(dialect.quote(&col));
            vals.push(format!(":ctx_{ctx_field}"));
            assigned.push(col);
        }
    }

    // The primary key. A `serial` PK is DB-generated: the INSERT *omits* the id column
    // entirely and the runtime reads the assigned value back (its `capture`). An
    // app-minted `id` (uuid/ulid, no SQL default) is bound as `:id[_step]` unless the
    // caller set it explicitly. A keyless (`@no_id`) model has no `id` column, and a
    // `@key(field)` model's key is a caller-supplied column set like any other assign.
    let serial_col = serial_return_col(model, dialect);
    let gen_id = if model.no_id || serial_col.is_some() || !model.key.is_empty() {
        None
    } else if !assigned.iter().any(|c| c == "id") {
        cols.insert(0, dialect.quote("id"));
        vals.insert(0, format!(":{id_param}"));
        Some(id_param.to_string())
    } else {
        None
    };

    let read_key = create_read_key(
        model,
        serial_col.as_deref(),
        &assigned,
        &vals,
        &value_by_field,
    );

    // `@created`/`@updated` are set on insert, unless the caller already did.
    for col in timestamp_cols(model, &[model.created.as_deref(), model.updated.as_deref()]) {
        if !assigned.contains(&col) {
            cols.push(dialect.quote(&col));
            vals.push("CURRENT_TIMESTAMP".to_string());
        }
    }

    let (tail, conflict_key) =
        upsert_tail_and_key(schema, decls, model, conflict, dialect, &value_by_field);

    // Row read-back (bound create / DB-generated return id) + its `RETURNING` columns.
    let (capture, returning) = build_capture(
        model,
        dialect,
        refs,
        claims_result,
        gen_id.as_deref(),
        serial_col.as_deref(),
        read_key.as_deref(),
        conflict_key.as_deref(),
    );

    LoweredWrite {
        header: format!("-- create {}\n", model.name),
        sql: insert_sql(dialect, model, &cols, &vals, &tail, &returning),
        model: model.name.clone(),
        gen_id,
        conflict_key,
        read_key,
        serial_col,
        creates: true,
        capture,
        wipe: false,
        bulk: None,
        real_delete: false,
    }
}

// ---------- bulk / structured shape-input create (BW1) ---------------------

/// Resolve a `create … from $param`'s input shape to `(from_model, body)`, via the param's
/// declared shape type. Sema already validated eligibility; this just re-reads the shape.
fn resolve_from_shape<'a>(cx: &LowerCx<'a>, param: &str) -> Option<(&'a str, &'a [ShapeField])> {
    let p = cx.params.iter().find(|p| p.name.node == param)?;
    let BaseType::Model(name) = &p.ty.as_ref()?.base else {
        return None;
    };
    cx.decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name.node => {
            Some((s.from.node.as_str(), s.body.as_slice()))
        }
        _ => None,
    })
}

/// Lower a structured shape-input `create` (`create Model from $row` / `create Model[] from
/// $rows`, BW1) to a [`BulkInsert`] column plan. The runtime materializes the chunked,
/// atomic multi-row INSERT from it — the row count is dynamic. Presence-driven: a column
/// the input shape names is written verbatim from the payload; an absent engine-managed
/// column (`id`, `@created`/`@updated`) is filled by the engine; `@scope` is *always*
/// injected from `$ctx`, even when the shape names it.
/// The presence-driven INSERT column plan for a structured shape-input create, plus the
/// DB-generated `serial` id column to `RETURNING` (if any). A column the shape names is
/// written verbatim from the payload (except a `@scope` column); an absent engine-managed
/// column is filled (mint / DB-gen / `now()`); `@scope` is always the caller's `$ctx`.
fn bulk_columns(
    cx: &LowerCx,
    model: &RModel,
    body: &[ShapeField],
) -> (Vec<BulkCol>, Option<String>) {
    let scope_terms: Vec<(String, String)> = cx
        .inject
        .iter()
        .find(|si| si.model == model.name)
        .map_or(Vec::new(), |si| si.terms.clone());
    let scope_cols: Vec<String> = scope_terms
        .iter()
        .map(|(f, _)| physical_col(model, f))
        .collect();

    // 1. Columns the input shape names — verbatim from the payload (a `@scope` column is
    //    skipped; step 2 injects it from `$ctx`).
    let mut columns: Vec<BulkCol> = Vec::new();
    let mut have: Vec<String> = Vec::new();
    for f in body {
        if let Some((col, src)) = named_bulk_col(model, f) {
            if !scope_cols.contains(&col) {
                bulk_push(&mut columns, &mut have, col, src);
            }
        } else if let ShapeField::Nest { field, .. } = f {
            if let Some(mem) = model.member(&field.node) {
                for (fk_col, part) in cx.schema.fk_columns(mem) {
                    let src = BulkSource::FkPart {
                        relation: field.node.clone(),
                        key_field: part.name.clone(),
                    };
                    bulk_push(&mut columns, &mut have, fk_col, src);
                }
            }
        }
    }

    // 2. `@scope` — always the caller's `$ctx.<field>`, identical for every row.
    for (field, ctx_field) in &scope_terms {
        let src = BulkSource::Ctx {
            ctx_field: ctx_field.clone(),
        };
        bulk_push(&mut columns, &mut have, physical_col(model, field), src);
    }

    // 3. The primary key when the shape does not name it: an app-minted `uuid`/`ulid` per
    //    row, or a DB-generated `serial` (omit — the DB assigns it). A `@key` / keyless
    //    model has no surrogate id (its key is an ordinary named column).
    let serial_col = serial_return_col(model, cx.dialect);
    if !model.no_id && model.key.is_empty() && !have.iter().any(|c| c == "id") {
        match model.pk_strategy() {
            Some(PkStrategy::Serial) => { /* DB-generated — omit */ }
            Some(PkStrategy::Ulid) => {
                bulk_push(
                    &mut columns,
                    &mut have,
                    "id".to_string(),
                    BulkSource::MintUlid,
                );
            }
            _ => bulk_push(
                &mut columns,
                &mut have,
                "id".to_string(),
                BulkSource::MintUuid,
            ),
        }
    }

    // 4. `@created`/`@updated` stamps the shape did not name → `CURRENT_TIMESTAMP`.
    for col in timestamp_cols(model, &[model.created.as_deref(), model.updated.as_deref()]) {
        bulk_push(&mut columns, &mut have, col, BulkSource::Now);
    }

    (columns, serial_col)
}

/// Append a bulk-insert column unless its physical column is already present (first source
/// wins — the shape-named value beats a later engine default).
fn bulk_push(columns: &mut Vec<BulkCol>, have: &mut Vec<String>, col: String, source: BulkSource) {
    if !have.contains(&col) {
        have.push(col.clone());
        columns.push(BulkCol {
            column: col,
            source,
        });
    }
}

/// The `(physical_col, BulkSource::Field)` a scalar input-shape field writes — a bare column
/// or a single-column rename. `None` for a relation nest (handled by the caller's FK
/// expansion) or any form sema rejected.
fn named_bulk_col(model: &RModel, f: &ShapeField) -> Option<(String, BulkSource)> {
    match f {
        ShapeField::Bare(id) => Some((
            physical_col(model, &id.node),
            BulkSource::Field {
                json_key: id.node.clone(),
                field: id.node.clone(),
            },
        )),
        ShapeField::Rename {
            out,
            value: ShapeValue::Path(p),
        } if p.segments.len() == 1 => {
            let field = p.segments[0].node.clone();
            Some((
                physical_col(model, &field),
                BulkSource::Field {
                    json_key: out.node.clone(),
                    field,
                },
            ))
        }
        _ => None,
    }
}

fn lower_bulk_create(
    cx: &LowerCx,
    model: &RModel,
    cf: &CreateFrom,
    _claims_result: bool,
) -> Option<LoweredWrite> {
    let (_from, body) = resolve_from_shape(cx, &cf.param.node)?;
    let dialect = cx.dialect;
    let (columns, serial_col) = bulk_columns(cx, model, body);
    let bulk = BulkInsert {
        model: model.name.clone(),
        table: dialect.quote_table(model.schema.as_deref(), &model.table),
        param: cf.param.node.clone(),
        bulk: cf.bulk,
        columns,
        returning: serial_col.into_iter().collect(),
    };
    Some(LoweredWrite {
        header: format!(
            "-- create {}{} from ${} (chunked multi-row INSERT — materialized by the runtime)\n",
            model.name,
            if cf.bulk { "[]" } else { "" },
            cf.param.node
        ),
        sql: bulk_review_sql(dialect, &bulk),
        model: model.name.clone(),
        gen_id: None,
        conflict_key: None,
        read_key: None,
        serial_col: None,
        creates: true,
        capture: None,
        wipe: false,
        bulk: Some(bulk),
        real_delete: false,
    })
}

/// A review-only single-row rendering of a bulk insert for `based gen sql` output (the
/// runtime never executes this — it materializes the real, chunked statement from the
/// [`BulkInsert`] plan). Placeholders name each column's per-row source.
fn bulk_review_sql(dialect: Dialect, bulk: &BulkInsert) -> String {
    let cols: Vec<String> = bulk
        .columns
        .iter()
        .map(|c| dialect.quote(&c.column))
        .collect();
    let vals: Vec<String> = bulk
        .columns
        .iter()
        .map(|c| match &c.source {
            BulkSource::Field { json_key, .. } => format!(":row.{json_key}"),
            BulkSource::FkPart {
                relation,
                key_field,
            } => format!(":row.{relation}.{key_field}"),
            BulkSource::MintUuid => ":mint(uuid)".to_string(),
            BulkSource::MintUlid => ":mint(ulid)".to_string(),
            BulkSource::Ctx { ctx_field } => format!(":ctx_{ctx_field}"),
            BulkSource::Now => "CURRENT_TIMESTAMP".to_string(),
        })
        .collect();
    let ret = if bulk.returning.is_empty() {
        String::new()
    } else {
        format!(
            " RETURNING {}",
            bulk.returning
                .iter()
                .map(|c| dialect.quote(c))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "INSERT INTO {} ({})\nVALUES ({}){ret};  -- repeated per row, chunked below the driver's bind limit\n",
        bulk.table,
        cols.join(", "),
        vals.join(", "),
    )
}

/// Build a create's row read-back plan and the `RETURNING` column list its INSERT carries.
/// A bound create captures the columns its siblings reference (`refs`); the return model's
/// create additionally captures its DB-generated id under `result_id` (an app-minted id is
/// known at plan time and needs no capture). On MySQL (no `INSERT … RETURNING`) the capture
/// carries a follow-up keyed `SELECT` and the `RETURNING` list is empty (D124).
#[allow(clippy::too_many_arguments)]
fn build_capture(
    model: &RModel,
    dialect: Dialect,
    refs: &[CaptureCol],
    claims_result: bool,
    gen_id: Option<&str>,
    serial_col: Option<&str>,
    read_key: Option<&[(String, String)]>,
    conflict_key: Option<&[(String, String)]>,
) -> (Option<Capture>, Vec<String>) {
    let mut cap_cols: Vec<CaptureCol> = refs.to_vec();
    if claims_result {
        if let Some(sc) = serial_col {
            let field = model
                .serial_key_member()
                .map_or_else(|| "id".to_string(), |m| m.name.clone());
            cap_cols.push(CaptureCol {
                bind: "result_id".to_string(),
                column: sc.to_string(),
                field,
            });
        }
    }
    if cap_cols.is_empty() {
        return (None, Vec::new());
    }
    // The distinct physical columns to read back, in first-seen order — the `RETURNING`
    // list (Postgres/SQLite/MariaDB) or the follow-up `SELECT` projection (MySQL).
    let mut cols: Vec<String> = Vec::new();
    for c in &cap_cols {
        if !cols.contains(&c.column) {
            cols.push(c.column.clone());
        }
    }
    if dialect == Dialect::MySql {
        let followup_select = followup_select_sql(
            dialect,
            model,
            &cols,
            gen_id,
            serial_col,
            read_key,
            conflict_key,
        );
        (
            Some(Capture {
                cols: cap_cols,
                followup_select: Some(followup_select),
            }),
            Vec::new(),
        )
    } else {
        (
            Some(Capture {
                cols: cap_cols,
                followup_select: None,
            }),
            cols,
        )
    }
}

/// The MySQL follow-up keyed `SELECT` that reads a just-inserted row's committed columns
/// back (MySQL has no `INSERT … RETURNING`). Keyed the same way the declared re-select is:
/// a DB-generated `serial` part on `LAST_INSERT_ID()` (plus any app-supplied composite
/// parts), an app-minted surrogate id on its `:id[_step]` bind, or a natural/keyless
/// unique column on its set value. Unbound `:name` SQL — the run stage binds it.
#[allow(clippy::too_many_arguments)]
fn followup_select_sql(
    dialect: Dialect,
    model: &RModel,
    cols: &[String],
    gen_id: Option<&str>,
    serial_col: Option<&str>,
    read_key: Option<&[(String, String)]>,
    conflict_key: Option<&[(String, String)]>,
) -> String {
    let projection = cols
        .iter()
        .map(|c| dialect.quote(c))
        .collect::<Vec<_>>()
        .join(", ");
    let mut wheres: Vec<String> = Vec::new();
    if let Some(sc) = serial_col {
        wheres.push(format!("{} = LAST_INSERT_ID()", dialect.quote(sc)));
        for (c, v) in read_key.unwrap_or(&[]) {
            wheres.push(format!("{} = {v}", dialect.quote(c)));
        }
    } else if let Some(gid) = gen_id {
        wheres.push(format!("{} = :{gid}", dialect.quote("id")));
    } else if let Some(pairs) = read_key.or(conflict_key) {
        for (c, v) in pairs {
            wheres.push(format!("{} = {v}", dialect.quote(c)));
        }
    }
    format!(
        "SELECT {projection} FROM {} WHERE {};\n",
        dialect.quote_table(model.schema.as_deref(), &model.table),
        wheres.join(" AND ")
    )
}

/// The upsert tail (`ON CONFLICT (cols) DO UPDATE SET …` / `ON DUPLICATE KEY UPDATE …`) and
/// the conflict-target read-back key (each target column's set value), or `("", None)` for a
/// plain create. The conflict key reads the winning row back on both the insert and conflict
/// paths.
fn upsert_tail_and_key(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: &RModel,
    conflict: Option<&OnConflict>,
    dialect: Dialect,
    value_by_field: &std::collections::HashMap<String, String>,
) -> (String, Option<Vec<(String, String)>>) {
    let Some(oc) = conflict else {
        return (String::new(), None);
    };
    let sets = conflict_update_sets(schema, decls, model, oc, dialect);
    let key: Vec<(String, String)> = oc
        .target
        .iter()
        .filter_map(|t| {
            value_by_field
                .get(&t.node)
                .map(|v| (physical_col(model, &t.node), v.clone()))
        })
        .collect();
    (upsert_tail(dialect, oc, model, &sets), Some(key))
}

/// A create with no generated id reads its row back by column(s) it set: a keyless
/// (`@no_id`) model's `(unique)` column, a single `@key(field)`'s column, or the full
/// composite `@key(f1, f2, …)` tuple. A composite key with a `serial` part reads back on its
/// *other* (app-supplied) parts — the serial part rides `:result_id` (the captured DB value).
/// Sema guarantees a keyless declared-shape return sets a unique column (E0264); a `@key`
/// model's non-serial key columns are required, always set.
fn create_read_key(
    model: &RModel,
    serial_col: Option<&str>,
    assigned: &[String],
    vals: &[String],
    value_by_field: &std::collections::HashMap<String, String>,
) -> Option<Vec<(String, String)>> {
    if let Some(sc) = serial_col {
        model
            .is_composite_key()
            .then(|| non_serial_key_pairs(model, sc, assigned, vals))
    } else if model.is_composite_key() {
        composite_read_key(model, assigned, vals)
    } else if model.no_id || !model.key.is_empty() {
        model.unique_cols.iter().find_map(|u| {
            value_by_field
                .get(u)
                .map(|v| vec![(physical_col(model, u), v.clone())])
        })
    } else {
        None
    }
}

/// The read-back key for a composite-`@key` create: every key column the create set, paired
/// with the value SQL it set. `None` if any key column went unset (an already-erroring
/// schema) so codegen never emits a half-keyed re-select.
fn composite_read_key(
    model: &RModel,
    assigned: &[String],
    vals: &[String],
) -> Option<Vec<(String, String)>> {
    let key: Vec<(String, String)> = model
        .key
        .iter()
        .filter_map(|f| {
            let col = physical_col(model, f);
            assigned
                .iter()
                .position(|c| c == &col)
                .map(|i| (col.clone(), vals[i].clone()))
        })
        .collect();
    (key.len() == model.key.len()).then_some(key)
}

/// The DB-generated PK column a `create` recovers from its INSERT (the deferred read-back
/// keys on it): the sole `serial` `id`, or a composite `@key`'s `serial` part on
/// Postgres/MariaDB. `None` otherwise — including a composite serial part on SQLite, which
/// has no auto-increment for a non-sole-PK column, so it is app-supplied there.
fn serial_return_col(model: &RModel, dialect: Dialect) -> Option<String> {
    if model.pk_is_db_generated() {
        return Some(physical_col(model, "id"));
    }
    model.serial_key_column().filter(|_| {
        matches!(
            dialect,
            Dialect::Postgres | Dialect::MariaDb | Dialect::MySql
        )
    })
}

/// A composite key's app-supplied parts (every key column except the DB-generated `serial`
/// one), each paired with the value SQL the create set — the parts that key the deferred
/// re-select alongside the captured serial value (`:result_id`).
fn non_serial_key_pairs(
    model: &RModel,
    serial_col: &str,
    assigned: &[String],
    vals: &[String],
) -> Vec<(String, String)> {
    model
        .key
        .iter()
        .filter_map(|f| {
            let col = physical_col(model, f);
            if col == serial_col {
                return None;
            }
            assigned
                .iter()
                .position(|c| c == &col)
                .map(|i| (col.clone(), vals[i].clone()))
        })
        .collect()
}

/// Assemble the `INSERT` statement text. A bound create (and a `serial` create's id)
/// reads its written row back via `RETURNING <cols>` on Postgres/SQLite/MariaDB (MySQL
/// has no `INSERT … RETURNING` — its read-back is a follow-up keyed `SELECT`, so `returning`
/// is empty there); a create that sets no columns uses the dialect's default-values form.
fn insert_sql(
    dialect: Dialect,
    model: &RModel,
    cols: &[String],
    vals: &[String],
    tail: &str,
    returning: &[String],
) -> String {
    let table = dialect.quote_table(model.schema.as_deref(), &model.table);
    let returning = if returning.is_empty() {
        String::new()
    } else {
        format!(
            " RETURNING {}",
            returning
                .iter()
                .map(|c| dialect.quote(c))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if cols.is_empty() {
        return match dialect {
            Dialect::Postgres | Dialect::Sqlite => {
                format!("INSERT INTO {table} DEFAULT VALUES{returning};\n")
            }
            Dialect::MariaDb | Dialect::MySql => {
                format!("INSERT INTO {table} () VALUES (){returning};\n")
            }
        };
    }
    format!(
        "INSERT INTO {table} ({})\nVALUES ({}){tail}{returning};\n",
        cols.join(", "),
        vals.join(", "),
    )
}

/// The `SET col = value` fragments for an upsert's `update` branch. Columns render **bare**
/// on both sides (a bare RHS column names the existing row on every dialect; a qualified
/// one is rejected/ambiguous in the conflict clause), reusing the ordinary assign lowering
/// (enum variants → wire literals, `hits = hits + 1` → the atomic arithmetic).
fn conflict_update_sets(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: &RModel,
    oc: &OnConflict,
    dialect: Dialect,
) -> Vec<String> {
    let mut sel = Select::new(schema, decls, model, dialect).with_bare_cols(true);
    oc.update
        .iter()
        .map(|a| {
            let col = physical_col(model, &a.col.node);
            let val = sel.assign_rhs(&a.value, model, &a.col.node);
            format!("{} = {val}", dialect.quote(&col))
        })
        .collect()
}

/// The per-dialect upsert clause appended to the INSERT: Postgres/SQLite carry the explicit
/// conflict-target column list, MariaDB does not (`ON DUPLICATE KEY UPDATE`).
fn upsert_tail(dialect: Dialect, oc: &OnConflict, model: &RModel, sets: &[String]) -> String {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => {
            format!("\nON DUPLICATE KEY UPDATE {}", sets.join(", "))
        }
        Dialect::Postgres | Dialect::Sqlite => {
            let target: Vec<String> = oc
                .target
                .iter()
                .map(|t| dialect.quote(&physical_col(model, &t.node)))
                .collect();
            format!(
                "\nON CONFLICT ({}) DO UPDATE SET {}",
                target.join(", "),
                sets.join(", ")
            )
        }
    }
}

// ---------- update ---------------------------------------------------------

fn lower_update<'a>(
    cx: &LowerCx<'a>,
    model: &RModel,
    where_: &Predicate,
    assigns: &'a [Assign],
    bindings: &HashMap<&'a str, BackCtx<'a>>,
) -> LoweredWrite {
    let mut sel = Select::new(cx.schema, cx.decls, model, cx.dialect)
        .with_bindings(bindings.clone())
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);
    let mut sets: Vec<String> = Vec::new();
    let mut assigned: Vec<String> = Vec::new();

    for a in assigns {
        let col = physical_col(model, &a.col.node);
        let val = sel.assign_rhs(&a.value, model, &a.col.node);
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

// ---------- delete / hard delete -------------------------------------------

/// Lower a `delete` / `hard delete`. `where_` is `None` for the whole-table wipe
/// `delete all` — every row (in scope). A soft model's plain `delete[ all]` tombstones
/// (UPDATE) rather than really deleting; `hard delete[ all]` and a plain model emit a real
/// DELETE. A `hard delete all` with no injected guard (unscoped / non-scoped, non-soft)
/// lowers to `TRUNCATE` on Postgres (transaction-safe there) — see [`Dialect::wipe_all`].
fn lower_delete(
    cx: &LowerCx,
    model: &RModel,
    where_: Option<&Predicate>,
    hard: bool,
) -> LoweredWrite {
    let wipe = where_.is_none();
    let mut sel = Select::new(cx.schema, cx.decls, model, cx.dialect)
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);

    // Soft model + plain `delete[ all]` -> tombstone UPDATE, never a real DELETE.
    if let (Some(sd), false) = (&model.soft_delete, hard) {
        let mut sets = vec![tombstone_set(&sel, model, sd, /* deleting = */ true)];
        if let Some(bump) = updated_bump(&sel, model, &[]) {
            sets.push(bump);
        }
        // `delete all` narrows by nothing user-supplied; only the injected live + scope
        // guards remain (tombstone every live row in scope).
        let mut wheres: Vec<String> = where_
            .map(|p| sel.predicate(p, model))
            .into_iter()
            .collect();
        inject_guards(&mut sel, model, &mut wheres, /* live = */ true);
        let header = if wipe {
            "-- delete all (soft): tombstone every row in scope\n"
        } else {
            "-- delete (soft): tombstone, never a real DELETE\n"
        };
        return LoweredWrite {
            header: header.to_string(),
            sql: update_stmt(&sel, model, &sets, &wheres),
            model: model.name.clone(),
            gen_id: None,
            conflict_key: None,
            read_key: None,
            serial_col: None,
            creates: false,
            capture: None,
            wipe,
            bulk: None,
            real_delete: false,
        };
    }

    // Plain model, or the loud `hard delete` opt-out -> real DELETE.
    let mut wheres: Vec<String> = where_
        .map(|p| sel.predicate(p, model))
        .into_iter()
        .collect();
    inject_guards(&mut sel, model, &mut wheres, /* live = */ false);

    // `[hard ]delete all` with no remaining WHERE (unscoped / non-scoped) is a true
    // whole-table wipe: `TRUNCATE` where transaction-safe (Postgres), else `DELETE FROM t`.
    // A scoped wipe keeps its scope predicate, so it stays a `DELETE FROM t WHERE <scope>`.
    let sql = if wipe && wheres.is_empty() {
        cx.dialect.wipe_all(&sel.qt(model))
    } else {
        delete_stmt(&sel, model, &wheres)
    };
    let header = match (hard, wipe) {
        (true, true) => "-- hard delete all: whole-table wipe\n",
        (true, false) => "-- hard delete: real DELETE (explicit soft-delete opt-out)\n",
        (false, true) => "-- delete all: whole-table wipe\n",
        (false, false) => "",
    };
    LoweredWrite {
        header: header.to_string(),
        sql,
        model: model.name.clone(),
        gen_id: None,
        conflict_key: None,
        read_key: None,
        serial_col: None,
        creates: false,
        capture: None,
        wipe,
        bulk: None,
        // A filtered real DELETE ack-checks its zero-rows-affected as a 404; a wipe does not.
        real_delete: !wipe,
    }
}

// ---------- restore --------------------------------------------------------

fn lower_restore(cx: &LowerCx, model: &RModel, where_: &Predicate) -> LoweredWrite {
    let mut sel = Select::new(cx.schema, cx.decls, model, cx.dialect)
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);
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
        Dialect::MariaDb | Dialect::MySql => sel.qcol(&sel.root_alias, col),
    }
}

/// `UPDATE t [join] SET ... WHERE ...`. A relation-reaching `where` seeds joins, which
/// differ by dialect: MySQL puts them inline (`UPDATE t JOIN j ON … SET …`), Postgres
/// moves the joined tables into a `FROM` list and folds the join `ON` into the `WHERE`
/// (`UPDATE t SET … FROM j WHERE <join-on> AND …`). Without joins both are the plain
/// single-table `UPDATE t SET … WHERE …`.
fn update_stmt(sel: &Select, model: &RModel, sets: &[String], wheres: &[String]) -> String {
    let mut s = format!("UPDATE {}", sel.qt(model));
    if sel.dialect == Dialect::Postgres {
        s.push_str(&format!("\nSET {}", sets.join(", ")));
        let mut wheres = wheres.to_vec();
        push_from_using(&mut s, sel, &mut wheres, "FROM");
        push_where(&mut s, &wheres);
    } else {
        push_joins(&mut s, sel.dialect, &sel.joins);
        s.push_str(&format!("\nSET {}", sets.join(", ")));
        push_where(&mut s, wheres);
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
            s.push_str(&format!("DELETE FROM {}", sel.qt(model)));
            let mut wheres = wheres.to_vec();
            push_from_using(&mut s, sel, &mut wheres, "USING");
            push_where(&mut s, &wheres);
        }
        _ if sel.joins.is_empty() => {
            s.push_str(&format!("DELETE FROM {}", sel.qt(model)));
            push_where(&mut s, wheres);
        }
        _ => {
            s.push_str(&format!(
                "DELETE {} FROM {}",
                sel.q(&sel.root_alias),
                sel.qt(model)
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
        .map(|j| {
            format!(
                "{} AS {}",
                sel.dialect.quote_table(j.schema.as_deref(), &j.table),
                sel.q(&j.alias)
            )
        })
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
