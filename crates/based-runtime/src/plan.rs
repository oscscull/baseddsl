//! Planning a query request: validate → thread `$ctx` → bind → pick the envelope.
//!
//! This is the runtime's core. It reads the signature (AST `Query` for the params,
//! `RQuery` for the inferred verb / cardinality / pagination and the `$ctx`
//! requirement bag) and the lowered SQL, and produces an executable [`QueryPlan`]:
//! positional statements + the [`Envelope`] the rows are shaped into.
//!
//! Binding uses the fact that codegen's placeholder *names* are unambiguous given
//! the schema: a declared param renders `:<param>`, a context field `:ctx_<field>`
//! (D11), offset pagination `:offset`. So the runtime assembles one value
//! environment from the validated inputs and lets [`crate::scan::to_positional`]
//! pull from it in SQL order.

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
}

impl Request {
    /// Convenience: a request whose args/ctx come from JSON objects (a non-object
    /// value is treated as empty — the wire layer will have rejected it already).
    pub fn new(
        callable: impl Into<String>,
        args: serde_json::Value,
        ctx: serde_json::Value,
    ) -> Self {
        Request {
            callable: callable.into(),
            args: args.as_object().cloned().unwrap_or_default(),
            ctx: ctx.as_object().cloned().unwrap_or_default(),
        }
    }
}

/// How the executed rows become the response body (queries.md / pagination.md).
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
}

/// A planned mutation: the write statements in execution order (all bound
/// positionally), run under one engine-owned transaction (principle 7).
#[derive(Debug, Clone)]
pub struct MutationPlan {
    pub name: String,
    pub stmts: Vec<Stmt>,
    /// The engine-generated `id` of the create matching the mutation's return model —
    /// the row the write response identifies. `None` when the mutation creates no such
    /// row (a pure update/delete, or a create whose `id` the caller set).
    pub result_id: Option<String>,
    /// The declared-shape re-select (D12): reads the created row back in the mutation's
    /// return shape (`:result_id` bound to [`result_id`](Self::result_id)) so the write
    /// response matches the client's decoded output type. `Some` exactly when
    /// `result_id` is — a mutation that creates its return row. `None` otherwise, and
    /// the response falls back to `{ id }` / `{}`.
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
}

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
    let mut env = Env::default();
    for p in &ast.params {
        env.insert(p.name.node.clone(), bind_param(p, req)?);
    }
    for c in &rq.ctx_requires {
        env.insert(format!("ctx_{}", c.field), bind_ctx(c, req)?);
    }
    if offset_paginated(ast) {
        env.insert("offset".to_string(), bind_offset(req)?);
    }

    // 2. Translate `:name` → `?` for the main and (optional) count statements.
    let main = env.bind(&low.sql)?;
    let count = low.count_sql.as_deref().map(|s| env.bind(s)).transpose()?;

    // 3. The response shape follows the query's inferred cardinality (queries.md).
    let envelope = match rq.verb {
        Verb::Get => Envelope::One,
        Verb::List if rq.paginated => Envelope::Page {
            with_count: count.is_some(),
        },
        Verb::List => Envelope::Many,
    };

    Ok(QueryPlan {
        name: req.callable.clone(),
        main,
        count,
        envelope,
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
    let mut env = Env::default();
    for p in &ast.params {
        env.insert(p.name.node.clone(), bind_param(p, req)?);
    }
    for c in &rm.ctx_requires {
        env.insert(format!("ctx_{}", c.field), bind_ctx(c, req)?);
    }

    // 2. Generate the engine `id` for each create (D1). Record the id of the first
    //    create matching the return model — the row the response identifies.
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

    // 4. The declared-shape re-select (D12), when the mutation creates its return row:
    //    bind `:result_id` to that create's engine id (already in `result_id`). Codegen
    //    emits the re-select under the same rule, so both are `Some` together.
    let ret_select = match (&low.ret_select, &result_id) {
        (Some(sql), Some(id)) => {
            env.insert("result_id".to_string(), SqlValue::Text(id.clone()));
            Some(env.bind(sql)?)
        }
        _ => None,
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
/// SQL placeholder order.
#[derive(Default)]
struct Env {
    values: std::collections::HashMap<String, SqlValue>,
}

impl Env {
    fn insert(&mut self, name: String, v: SqlValue) {
        self.values.insert(name, v);
    }

    /// Rewrite one statement to positional form, resolving each `:name` from the
    /// environment. An unresolved name is an internal invariant break, not a user
    /// error (every declared bind was inserted above).
    fn bind(&self, sql: &str) -> Result<Stmt, PlanError> {
        let (sql, params) = to_positional(sql, |name| self.values.get(name).cloned())
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
/// shape-coerced) — strict per-column typing of untyped params is a later slice.
fn param_family(p: &Param) -> (Family, bool) {
    match &p.ty {
        Some(t) => {
            let family = match &t.base {
                BaseType::Primitive(prim) => Family::of(*prim),
                // A relation param carries the target's key (D1): a uuid string.
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
        // A relation-typed context field carries the model's key (D1).
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
/// the first page (offset 0), never an error (pagination.md — the default is safe).
fn bind_offset(req: &Request) -> Result<SqlValue, PlanError> {
    match req.args.get("offset") {
        Some(v) => coerce(v, Family::Int, false).map_err(|e| bad_arg("offset", e)),
        None => Ok(SqlValue::Int(0)),
    }
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
        DefaultVal::Lit(Literal::Float(f)) => SqlValue::Float(*f),
        DefaultVal::Lit(Literal::Bool(b)) => SqlValue::Bool(*b),
        DefaultVal::Lit(Literal::Null) => SqlValue::Null,
        DefaultVal::Func(_) => SqlValue::Null,
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
