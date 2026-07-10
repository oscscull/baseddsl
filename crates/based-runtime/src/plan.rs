//! Planning a query request: validate → thread `$ctx` → bind → pick the envelope.
//!
//! This is the runtime's core. It reads the signature (AST `Query` for the params,
//! `RQuery` for the inferred verb / cardinality / pagination and the `$ctx`
//! requirement bag) and the lowered SQL, and produces an executable [`QueryPlan`]:
//! positional statements + the [`Envelope`] the rows are shaped into.
//!
//! Binding uses the fact that codegen's placeholder names are unambiguous given the
//! schema: a declared param renders `:<param>`, a context field `:ctx_<field>`, offset
//! pagination `:offset`. So the runtime assembles one value environment from the
//! validated inputs and lets [`crate::scan::to_positional`] pull from it in SQL order.

use based_ast::{BaseType, Decl, DefaultVal, Literal, Mutation, Param, Query, Verb};
use based_sema::{CtxField, CtxReq};

use crate::id::IdGen;
use crate::load::Compiled;
use crate::scan::to_positional;
use crate::value::{coerce, CoerceError, Family, SqlValue};

/// A wire request: which signature, the JSON args, and the request `$ctx`.
#[derive(Debug, Clone)]
pub struct Request {
    pub callable: String,
    pub args: serde_json::Map<String, serde_json::Value>,
    pub ctx: serde_json::Map<String, serde_json::Value>,
    /// An optional idempotency key for a mutation retry: the caller attaches a stable key
    /// so the engine runs the write body at most once per key. Request metadata, supplied
    /// out of band (the `Idempotency-Key` header), never the JSON body — the same
    /// trusted-edge discipline as `$ctx`, and never a schema field. `None` → run every time
    /// (the default). Ignored by queries.
    pub idempotency_key: Option<String>,
}

impl Request {
    /// Convenience: a request whose args/ctx come from JSON objects (a non-object
    /// value is treated as empty — the wire layer will have rejected it already), with
    /// no idempotency key. Use [`Request::with_idempotency_key`] to attach one.
    pub fn new(
        callable: impl Into<String>,
        args: serde_json::Value,
        ctx: serde_json::Value,
    ) -> Self {
        Request {
            callable: callable.into(),
            args: args.as_object().cloned().unwrap_or_default(),
            ctx: ctx.as_object().cloned().unwrap_or_default(),
            idempotency_key: None,
        }
    }

    /// Attach a mutation idempotency key. A blank/whitespace-only key is treated as absent
    /// (a header set to `""` is not a real key), so an empty header never claims a store
    /// slot.
    pub fn with_idempotency_key(mut self, key: Option<String>) -> Self {
        self.idempotency_key = key.filter(|k| !k.trim().is_empty());
        self
    }

    /// A stable hash of this request's payload — its args and `$ctx` — for the idempotency
    /// store. A genuine retry of the same request produces the same fingerprint (so the
    /// stored response replays); a caller who reuses one key for a different request
    /// produces a different one, which the store rejects rather than silently answering
    /// with the first request's result.
    ///
    /// Only the payload is fingerprinted, not the callable or the key — the store already
    /// scopes an entry by `(callable, key)`, so the fingerprint's job is purely to detect a
    /// payload change under a reused `(callable, key)`. The idempotency key itself is
    /// deliberately excluded (it *is* the entry's key). `serde_json::Map` is BTreeMap-backed
    /// (sorted keys), so `to_string` is a canonical serialization and the hash is stable
    /// across attempts.
    pub fn fingerprint(&self) -> crate::idempotency::Fingerprint {
        // FNV-1a over the canonical JSON of (args, ctx). A field-count prefix separates the
        // two maps so that moving a field from args to ctx changes the hash (no ambiguous
        // concatenation). FNV is stable across releases (unlike `DefaultHasher`), which a
        // durable multi-instance store relies on.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a 64-bit offset basis.
        for part in [&self.args, &self.ctx] {
            let s = serde_json::Value::Object(part.clone()).to_string();
            for b in s.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime.
            }
            // A separator byte between the two maps.
            h ^= 0xff;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// How the executed rows become the response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Envelope {
    /// `get` — at most one row → `Option<T>` (JSON object or `null`).
    One,
    /// `list` — every row → a JSON array.
    Many,
    /// paginated `list` → the `{ rows, cursor }` envelope; `with_count` adds `total`.
    Page { with_count: bool },
}

/// One executable statement: positional SQL + its bound values, in `?` order.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

/// A planned query: the main statement, an optional live-row count (for a
/// `with count` page), and the response envelope.
#[derive(Debug, Clone)]
pub struct QueryPlan {
    pub name: String,
    pub main: Stmt,
    pub count: Option<Stmt>,
    pub envelope: Envelope,
    /// Keyset descriptor for a cursor-paginated `list`, else `None`. The run stage reads
    /// the last row's hidden `__keyset_<i>` columns to mint the next cursor and strips them
    /// from the response.
    pub keyset: Option<KeysetPlan>,
}

/// What the run stage needs to finish a keyset page: how many sort-key columns the
/// cursor carries (`__keyset_0..keys`) and the page size, so a full page yields a
/// "more" cursor and a short page (the last page) yields none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeysetPlan {
    pub keys: usize,
    pub page_size: u64,
}

/// A planned mutation: the write statements in execution order (all bound
/// positionally), run under one engine-owned transaction.
#[derive(Debug, Clone)]
pub struct MutationPlan {
    pub name: String,
    pub stmts: Vec<Stmt>,
    /// The engine-generated `id` of the create matching the mutation's return model —
    /// the row the write response identifies. `None` when the mutation creates no such
    /// row (a pure update/delete, or a create whose `id` the caller set).
    pub result_id: Option<String>,
    /// The declared-shape re-select: reads the written row back in the mutation's return
    /// shape so the write response matches the client's decoded output type. Keyed either
    /// on the created row's id (`:result_id` = [`result_id`](Self::result_id)) or on an
    /// update/soft-delete/restore's own `where` (its params already bound). `None` only
    /// when the row does not survive the write (a real DELETE), where the response falls
    /// back to `{}`.
    pub ret_select: Option<Stmt>,
}

/// Why a request could not be planned — all boundary failures, before any SQL.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    /// No query with this name.
    UnknownQuery(String),
    /// No mutation with this name.
    UnknownMutation(String),
    /// A required arg was absent (and had no default).
    MissingArg(String),
    /// An arg was present but the wrong JSON type for its param.
    BadArg {
        name: String,
        expected: Family,
        got: String,
    },
    /// A required `$ctx.<field>` was absent from the request context.
    MissingCtx(String),
    /// A `$ctx.<field>` was present but the wrong JSON type.
    BadCtx {
        field: String,
        expected: Family,
        got: String,
    },
    /// The SQL referenced a `:name` the runtime could not resolve — an internal
    /// invariant break (codegen emitted a placeholder the planner did not bind).
    UnboundPlaceholder(String),
    /// A keyset `cursor` arg was present but malformed/tampered/of the wrong arity. The
    /// caller sent a bad cursor — a boundary error.
    BadCursor(String),
}

impl PlanError {
    /// The stable, machine-readable code for this failure — the single source of truth for
    /// the wire `error.code` (`serve`) and any library consumer that branches on the class
    /// of failure rather than the message text. Stable across releases.
    pub fn code(&self) -> &'static str {
        use PlanError::*;
        match self {
            UnknownQuery(_) => "unknown_query",
            UnknownMutation(_) => "unknown_mutation",
            MissingArg(_) => "missing_arg",
            BadArg { .. } => "bad_arg",
            MissingCtx(_) => "missing_ctx",
            BadCtx { .. } => "bad_ctx",
            UnboundPlaceholder(_) => "internal",
            BadCursor(_) => "bad_cursor",
        }
    }
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use PlanError::*;
        match self {
            UnknownQuery(n) => write!(f, "no query `{n}`"),
            UnknownMutation(n) => write!(f, "no mutation `{n}`"),
            MissingArg(n) => write!(f, "missing argument `{n}`"),
            BadArg {
                name,
                expected,
                got,
            } => write!(
                f,
                "argument `{name}`: expected {}, got {got}",
                expected.label()
            ),
            MissingCtx(field) => write!(f, "missing request context `$ctx.{field}`"),
            BadCtx {
                field,
                expected,
                got,
            } => write!(
                f,
                "context `$ctx.{field}`: expected {}, got {got}",
                expected.label()
            ),
            UnboundPlaceholder(n) => {
                write!(f, "unbound placeholder `:{n}` (codegen/planner mismatch)")
            }
            BadCursor(msg) => write!(f, "invalid cursor: {msg}"),
        }
    }
}

impl std::error::Error for PlanError {}

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

    // 1. Assemble the value environment: params, then `$ctx`, then pagination.
    let mut env = Env::new(compiled.dialect);
    for p in &ast.params {
        env.insert(p.name.node.clone(), bind_param(p, req)?);
    }
    for c in &rq.ctx_requires {
        env.insert(format!("ctx_{}", c.field), bind_ctx(c, req)?);
    }
    if offset_paginated(ast) {
        env.insert("offset".to_string(), bind_offset(req)?);
    }
    // Keyset pagination: decode the opaque `cursor` arg into the sort-key values
    // codegen's `:keyset_<i>` placeholders compare against, and flip
    // `:keyset_active`. Absent cursor = the first page: `:keyset_active = 0` short-
    // circuits the comparison to a no-op, and the `:keyset_<i>` bind to NULL (never
    // consulted). `low.keyset` is the codegen-authoritative key count.
    if let Some(n) = low.keyset {
        bind_cursor(&mut env, req, n)?;
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
    let keyset = low.keyset.map(|keys| KeysetPlan {
        keys,
        page_size: page_size(ast).unwrap_or(u64::MAX),
    });

    Ok(QueryPlan {
        name: req.callable.clone(),
        main,
        count,
        envelope,
        keyset,
    })
}

/// Plan a mutation request against a compiled project. Validates the args + `$ctx`
/// (exactly like a query), then generates the engine `id` for every `create` and
/// binds every write statement positionally. The generated ids are seeded into the
/// value environment *before* binding, so a `^.id` back-reference — which lowers to
/// the prior create's `:id_<step>` — resolves to the same value the INSERT used.
pub fn plan_mutation(
    compiled: &Compiled,
    req: &Request,
    id_gen: &mut dyn IdGen,
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

    // 1. Assemble the value environment: params, then `$ctx` (no pagination on a write).
    let mut env = Env::new(compiled.dialect);
    for p in &ast.params {
        env.insert(p.name.node.clone(), bind_param(p, req)?);
    }
    for c in &rm.ctx_requires {
        env.insert(format!("ctx_{}", c.field), bind_ctx(c, req)?);
    }

    // 2. Generate the engine `id` for each create. Record the id of the first create
    //    matching the return model — the row the response identifies.
    let mut result_id = None;
    for w in &low.stmts {
        if let Some(bind) = &w.gen_id {
            let id = id_gen.next_id();
            if result_id.is_none() && w.model == rm.ret_model {
                result_id = Some(id.clone());
            }
            env.insert(bind.clone(), SqlValue::Text(id));
        }
    }

    // 3. Bind every write statement to positional form, in execution order.
    let stmts = low
        .stmts
        .iter()
        .map(|w| env.bind(&w.sql))
        .collect::<Result<Vec<_>, _>>()?;

    // 4. The declared-shape re-select: bind it whenever codegen emitted one (codegen and
    //    this planner apply the same survives-the-write rule, so they agree). A create-keyed
    //    re-select needs `:result_id` = this create's engine id; a where-keyed one
    //    (update/soft-delete/restore) reuses the write's own params/`$ctx`, already in
    //    `env`. Seeding `:result_id` only when a create produced one is harmless for the
    //    where-keyed form — `to_positional` binds only the placeholders each statement carries.
    let ret_select = match &low.ret_select {
        Some(sql) => {
            if let Some(id) = &result_id {
                env.insert("result_id".to_string(), SqlValue::Text(id.clone()));
            }
            Some(env.bind(sql)?)
        }
        None => None,
    };

    Ok(MutationPlan {
        name: req.callable.clone(),
        stmts,
        result_id,
        ret_select,
    })
}

// ---------- value environment ---------------------------------------------

/// Named bind values gathered from the validated request; `bind` pulls from it in
/// SQL placeholder order. Carries the target `dialect` so the positional rewrite emits
/// the right placeholder form (`?` vs `$n`).
struct Env {
    dialect: based_codegen::Dialect,
    values: std::collections::HashMap<String, SqlValue>,
}

impl Env {
    fn new(dialect: based_codegen::Dialect) -> Self {
        Env {
            dialect,
            values: std::collections::HashMap::new(),
        }
    }

    fn insert(&mut self, name: String, v: SqlValue) {
        self.values.insert(name, v);
    }

    /// Rewrite one statement to positional form, resolving each `:name` from the
    /// environment. An unresolved name is an internal invariant break, not a user
    /// error (every declared bind was inserted above).
    fn bind(&self, sql: &str) -> Result<Stmt, PlanError> {
        let (sql, params) = to_positional(sql, self.dialect, |name| self.values.get(name).cloned())
            .map_err(PlanError::UnboundPlaceholder)?;
        Ok(Stmt { sql, params })
    }
}

// ---------- per-input binding ---------------------------------------------

/// Bind one signature param: use the supplied arg (coerced to the param's family),
/// or its default, or `null` if optional — else it is missing.
fn bind_param(p: &Param, req: &Request) -> Result<SqlValue, PlanError> {
    let (family, optional) = param_family(p);
    match req.args.get(&p.name.node) {
        Some(v) => coerce(v, family, optional).map_err(|e| bad_arg(&p.name.node, e)),
        None => {
            if let Some(dv) = &p.default {
                return Ok(default_value(dv));
            }
            if optional {
                return Ok(SqlValue::Null);
            }
            Err(PlanError::MissingArg(p.name.node.clone()))
        }
    }
}

/// The coercion family + nullability of a param. A model-typed or untyped param
/// keeps things loose (a relation key is a uuid string; an untyped param is
/// shape-coerced); untyped params are not strictly per-column typed.
fn param_family(p: &Param) -> (Family, bool) {
    match &p.ty {
        Some(t) => {
            let family = match &t.base {
                BaseType::Primitive(prim) => Family::of(*prim),
                // A relation param carries the target's key: a uuid string.
                BaseType::Model(_) => Family::Text,
            };
            (family, t.optional || p.default.is_some())
        }
        // Untyped: coerce by JSON shape. Optional only if it has a default.
        None => (Family::Any, p.default.is_some()),
    }
}

/// Bind one `$ctx.<field>` requirement from the request context. Always required —
/// the callable cannot run without the context it reads.
fn bind_ctx(c: &CtxReq, req: &Request) -> Result<SqlValue, PlanError> {
    let family = match &c.ty {
        CtxField::Scalar(prim) => Family::of(*prim),
        // A relation-typed context field carries the model's key.
        CtxField::Relation(_) => Family::Text,
    };
    match req.ctx.get(&c.field) {
        Some(v) => coerce(v, family, false).map_err(|e| PlanError::BadCtx {
            field: c.field.clone(),
            expected: e.expected,
            got: e.got,
        }),
        None => Err(PlanError::MissingCtx(c.field.clone())),
    }
}

/// Bind the `:offset` of an offset page. The client sends `offset`; absence means
/// the first page (offset 0), never an error (the default is safe).
fn bind_offset(req: &Request) -> Result<SqlValue, PlanError> {
    match req.args.get("offset") {
        Some(v) => coerce(v, Family::Int, false).map_err(|e| bad_arg("offset", e)),
        None => Ok(SqlValue::Int(0)),
    }
}

/// Bind a keyset page's cursor placeholders (`:keyset_active` + `:keyset_0..n`). The
/// caller sends the opaque `cursor` arg; absence is the first page (`:keyset_active =
/// 0`, the comparison a no-op, the value placeholders NULL — never consulted). A
/// present cursor is decoded + validated into its `n` sort-key values (`cursor.rs`); a
/// bad one is a `BadCursor` boundary error, not a silent empty page.
fn bind_cursor(env: &mut Env, req: &Request, n: usize) -> Result<(), PlanError> {
    match req.args.get("cursor").filter(|v| !v.is_null()) {
        Some(serde_json::Value::String(s)) => {
            let vals = crate::cursor::decode(s, n).map_err(|e| PlanError::BadCursor(e.0))?;
            env.insert("keyset_active".into(), SqlValue::Int(1));
            for (i, v) in vals.into_iter().enumerate() {
                env.insert(format!("keyset_{i}"), v);
            }
        }
        Some(_) => return Err(PlanError::BadCursor("cursor must be a string".into())),
        None => {
            env.insert("keyset_active".into(), SqlValue::Int(0));
            for i in 0..n {
                env.insert(format!("keyset_{i}"), SqlValue::Null);
            }
        }
    }
    Ok(())
}

// ---------- helpers --------------------------------------------------------

fn bad_arg(name: &str, e: CoerceError) -> PlanError {
    PlanError::BadArg {
        name: name.to_string(),
        expected: e.expected,
        got: e.got,
    }
}

/// A literal default → its bound value. A `now()` default has no request-time value
/// (it is a write-time engine concern) → `Null` here; query params default to
/// literals in practice.
fn default_value(dv: &DefaultVal) -> SqlValue {
    match dv {
        DefaultVal::Lit(Literal::Str(s)) => SqlValue::Text(s.clone()),
        DefaultVal::Lit(Literal::Int(i)) => SqlValue::Int(*i),
        // A fractional literal is carried as exact text (a decimal binds as a string); a
        // float default's text parses back to a number.
        DefaultVal::Lit(Literal::Decimal(s)) => SqlValue::Text(s.clone()),
        DefaultVal::Lit(Literal::Bool(b)) => SqlValue::Bool(*b),
        DefaultVal::Lit(Literal::Null) => SqlValue::Null,
        DefaultVal::Func(_) => SqlValue::Null,
        // An enum default is its wire variant string.
        DefaultVal::Variant(v) => SqlValue::Text(v.node.clone()),
    }
}

/// Whether the query paginates by offset (its lowered SQL carries `:offset`).
fn offset_paginated(ast: &Query) -> bool {
    use based_ast::{Clause, QueryBody};
    let clauses: &[Clause] = match &ast.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare => &[],
    };
    clauses
        .iter()
        .any(|c| matches!(c, Clause::Page(p) if p.offset))
}

/// The `page (...)` size of a paginated query (`None` when it does not paginate).
fn page_size(ast: &Query) -> Option<u64> {
    use based_ast::{Clause, QueryBody};
    let clauses: &[Clause] = match &ast.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare => &[],
    };
    clauses.iter().find_map(|c| match c {
        Clause::Page(p) => Some(p.size),
        _ => None,
    })
}

/// Find a query decl by name.
fn find_query<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Query> {
    decls.iter().find_map(|d| match d {
        Decl::Query(q) if q.name.node == name => Some(q),
        _ => None,
    })
}

/// Find a mutation decl by name.
fn find_mutation<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Mutation> {
    decls.iter().find_map(|d| match d {
        Decl::Mutation(m) if m.name.node == name => Some(m),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The fingerprint is stable for the same payload, differs when args or `$ctx` change,
    /// and is invariant to the idempotency key (the store scopes by key already — the
    /// fingerprint's only job is to detect a payload change under a reused key).
    #[test]
    fn fingerprint_tracks_the_payload_not_the_key() {
        let base = Request::new("m", json!({ "a": 1, "b": "x" }), json!({ "org": "o-1" }));

        // Same payload → same fingerprint, whatever the key.
        let same = base
            .clone()
            .with_idempotency_key(Some("different-key".into()));
        assert_eq!(base.fingerprint(), same.fingerprint());

        // Key order in the JSON object does not matter (BTreeMap-backed, sorted).
        let reordered = Request::new("m", json!({ "b": "x", "a": 1 }), json!({ "org": "o-1" }));
        assert_eq!(base.fingerprint(), reordered.fingerprint());

        // A different arg value → a different fingerprint.
        let arg_changed = Request::new("m", json!({ "a": 2, "b": "x" }), json!({ "org": "o-1" }));
        assert_ne!(base.fingerprint(), arg_changed.fingerprint());

        // A different `$ctx` → a different fingerprint.
        let ctx_changed = Request::new("m", json!({ "a": 1, "b": "x" }), json!({ "org": "o-2" }));
        assert_ne!(base.fingerprint(), ctx_changed.fingerprint());

        // Moving a field between args and ctx changes the hash (the separator prevents an
        // ambiguous concatenation collapsing the two maps).
        let a = Request::new("m", json!({ "x": 1 }), json!({}));
        let b = Request::new("m", json!({}), json!({ "x": 1 }));
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
