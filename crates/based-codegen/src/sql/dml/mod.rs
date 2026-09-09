//! SQL DML generation (read side): a `query` lowers to a parameterized SELECT.
//!
//! This is where the headline soft-delete guarantee becomes real:
//! the tombstone predicate is injected into every generated SELECT — on the root
//! table (in `WHERE`) and on every joined table (in its `ON`, so a `LEFT JOIN`
//! stays a left join). `@scope` rides the same injection path. The user
//! never writes either; both are compiler primitives.
//!
//! ## What a query lowers to
//! - FROM the target model's table (aliased by its table name).
//! - Projection from the return **shape**: bare fields are local columns, `out =
//!   path` reaches across relations (each relation step becomes a JOIN), `out =
//!   raw`…`` is an inline expression. A bare-model return projects every stored
//!   column. A to-one relation `field { … }` nests the target's projected columns
//!   under a `field`-prefixed alias (`field.<col>`, [`NEST_SEP`]-joined), which the
//!   runtime reassembles into a sub-object. A to-**many** relation `field { … }`
//!   aggregates the child rows into a JSON-array column (`field[]`, [`ARRAY_MARK`])
//!   via a correlated subquery + per-dialect JSON aggregation, which the runtime parses
//!   into an array of sub-objects; self-referential to-many (`invited_users`) works
//!   because the subquery aliases the child table distinctly from the outer row.
//! - WHERE from the query filter (bare-param same-name equality, per-param
//!   bindings, or an explicit `where`), then the injected soft-delete + scope.
//!   A named-filter call in `where` is inlined: its body is substituted with the
//!   call args and lowered against the call-site model (the codegen twin of the sema check).
//! - ORDER from the sort cascade (query `order` > model `@sort`); keyset queries
//!   get the unique `id` tiebreaker appended, shown not written.
//! - LIMIT / OFFSET from `page (...)`; `with count` emits a second COUNT(*). A keyset
//!   page also carries the lexicographic cursor predicate (guarded by `:keyset_active`,
//!   a no-op on page 1) + hidden `__keyset_<i>` cursor-basis columns.
//!
//! ## Parameter placeholders
//! Signature inputs render as `:name` named placeholders (`$ctx.org` -> `:ctx_org`).
//! The runtime binds them to the driver's positional form (`?` on MySQL/MariaDB/SQLite,
//! `$n` on Postgres). Named placeholders keep the emitted SQL legible — readable
//! over terse.
//!
//! ## Dialects
//! The SELECT text branches on the [`crate::Dialect`]: identifier quoting (`` `x` `` vs
//! `"x"`), the bare-bool literal (`= TRUE` vs SQLite's `= 1`), and JSON containment
//! (`has` -> MySQL's `MEMBER OF` vs Postgres's `@>`). Everything else is portable.
//!
//! ## To-many nested arrays (`items { … }`, self-referential `invited_users`)
//! A to-many nest lowers to a **correlated subquery** in the SELECT list, not a join:
//! `(SELECT <json-agg>(<json-object of the element body>) FROM <child> AS <s-alias>
//! WHERE <child.back_fk> = <outer>.id AND <child soft-delete/scope>)`. The child gets a
//! distinct `s<n>_<table>` alias, so a **self-referential** edge (`User.invited_users`
//! joined to `User`) never collides with the outer row. The element body recurses through
//! [`Select::json_object_expr`] — scalars/reaches become `'key', value` pairs, a to-one
//! nest a nested JSON object, a to-many nest a nested correlated subquery — so nesting
//! composes to any depth. The array's element order follows the sort cascade for the
//! traversal — the edge's relation `@sort`, else the child model's `@sort` — emitted as
//! an ORDER BY inside the JSON aggregate (all three dialects support the ordered form);
//! with neither declared the order stays unspecified.

use std::collections::{HashMap, HashSet};

pub(crate) use based_ast::*;
use based_sema::{
    CheckedSchema, EnumValue, MemberKind, REnum, RMember, RModel, RQuery, ScopeInject, SoftDelete,
    SoftMode,
};

use crate::Dialect;

mod aggregate;
mod joins;
mod nest;
mod order;
mod outputs;
mod predicate;
mod project;
mod scope;
mod select;
mod types;
mod util;
mod value;

pub(crate) use aggregate::*;
pub(crate) use joins::*;
pub(crate) use order::*;
pub(crate) use outputs::*;
pub(crate) use predicate::*;
pub(crate) use project::*;
pub(crate) use select::*;
pub(crate) use types::*;
pub(crate) use util::*;
pub(crate) use value::*;

pub use util::{ARRAY_MARK, KEYSET_PREFIX, NEST_PRESENT, NEST_SEP};

/// Render every query in the schema as a parameterized SELECT (mutations join in a
/// later increment). Statements are separated by blank lines, in declaration order.
pub fn dml(schema: &CheckedSchema, decls: &[Decl], dialect: Dialect) -> String {
    // The SELECT text now branches on the dialect: identifier quoting (`` `x` `` vs
    // `"x"`), the bare-bool literal, and JSON containment (`MEMBER OF` vs `@>`). The
    // one lowering below is shared with the runtime, so the emitted and executed SQL
    // can never disagree per dialect.
    let queries: HashMap<&str, &RQuery> = schema
        .queries
        .iter()
        .map(|q| (q.name.as_str(), q))
        .collect();

    let mut out = String::new();
    out.push_str(&format!(
        "-- Generated by `based gen sql` (dialect: {}). Do not edit by hand.\n",
        dialect.name()
    ));
    out.push_str("-- Query templates: `:name` placeholders are bound by the generated client.\n");
    for decl in decls {
        if let Decl::Query(q) = decl {
            if let Some(rq) = queries.get(q.name.node.as_str()) {
                out.push('\n');
                out.push_str(&render_query(schema, decls, q, rq, dialect));
            }
        }
    }
    out
}

// ---------- per-query lowering --------------------------------------------

/// A query lowered to its structured SQL: the primary SELECT plus, for a
/// `with count` page, the live-row COUNT. Both carry the `:name` placeholders
/// verbatim — the runtime binds them; the text emitter frames them with
/// `-- query` headers (`dml`). This is the one lowering; `render_query` and the
/// runtime both read it, so the SQL and its bind surface can never drift.
#[derive(Debug, Clone)]
pub struct LoweredQuery {
    pub name: String,
    /// Primary SELECT, ending in `;\n`, no comment header.
    pub sql: String,
    /// The live-row `COUNT(*)` SELECT for a `with count` page, else `None`.
    pub count_sql: Option<String>,
    /// For a **keyset** page (paginated, not `offset`): the sort-key columns' primitive
    /// types, in sort order. The SELECT carries `<key> AS __keyset_<i>` hidden columns
    /// (`0..n`) so the runtime can read the last row's cursor basis, and
    /// `:keyset_active` + `:keyset_<i>` placeholders it binds from the incoming cursor —
    /// each cursor value re-binds as its column's own primitive (a typed bind, which a
    /// binary-parameter driver requires). `None` for a non-paginated or
    /// offset-paginated query. One source of the keyset convention: codegen emits it,
    /// the runtime reads it.
    pub keyset: Option<Vec<Primitive>>,
    /// Output field-paths ([`NEST_SEP`]/[`ARRAY_MARK`]-joined, relative to one result row)
    /// of every `json`-typed leaf this projection produces. A `json` column stores JSON as
    /// text (SQLite/MariaDB), so a driver reads it back as a *string*; the runtime parses
    /// the value at each of these paths into structured JSON, so a `json` field reads back
    /// as the object/array it was written as (round-trips), never a double-encoded string.
    pub json_paths: Vec<String>,
}

/// Lower every query in the schema to its structured SQL, in declaration order.
/// The in-process runtime consumes this directly (no serialized artifact).
pub fn lower_queries(
    schema: &CheckedSchema,
    decls: &[Decl],
    dialect: Dialect,
) -> Vec<LoweredQuery> {
    let queries: HashMap<&str, &RQuery> = schema
        .queries
        .iter()
        .map(|q| (q.name.as_str(), q))
        .collect();
    decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Query(q) => queries
                .get(q.name.node.as_str())
                .map(|rq| lower_query(schema, decls, q, rq, dialect)),
            _ => None,
        })
        .collect()
}

/// Text emitter for one query: the SQL body framed with `-- query` comment
/// headers (the `based gen sql` surface). Delegates the SQL to `lower_query`.
fn render_query(
    schema: &CheckedSchema,
    decls: &[Decl],
    q: &Query,
    rq: &RQuery,
    dialect: Dialect,
) -> String {
    let low = lower_query(schema, decls, q, rq, dialect);
    let mut out = format!("-- query {}\n{}", low.name, low.sql);
    if let Some(count) = &low.count_sql {
        out.push('\n');
        out.push_str(&format!("-- query {} (count)\n{}", low.name, count));
    }
    out
}

/// The single query lowering: builds the primary SELECT (and count SELECT) as
/// header-free SQL. Both `render_query` (text) and the runtime read this.
fn lower_query(
    schema: &CheckedSchema,
    decls: &[Decl],
    q: &Query,
    rq: &RQuery,
    dialect: Dialect,
) -> LoweredQuery {
    let root = schema.model(&rq.target).expect("target resolved by sema");

    // A whole-query raw body IS the statement: text verbatim, `${param}` → `:param`
    // (bound like any placeholder), `{table}`/`{id}` → the target model's table.
    // Nothing engine-built composes — no injected soft-delete/scope, no sort
    // cascade, no pagination; the declared shape types the result columns by name.
    if let QueryBody::Raw(raw) = &q.body {
        let body = render_raw(dialect, raw, &root.table, &root.table);
        let body = body.trim_end().trim_end_matches(';').trim_end();
        return LoweredQuery {
            name: q.name.node.clone(),
            sql: format!("{body};\n"),
            count_sql: None,
            keyset: None,
            json_paths: Vec::new(),
        };
    }
    // `unscoped` opts the whole query out of scope handling — the joined
    // tables' `@scope` as well as the root's, kept in one decision.
    let mut sel = Select::new(schema, decls, root, dialect)
        .with_scope_inject(q.unscoped.is_none())
        .with_scope_terms(&rq.scope_inject);

    // An aggregate return shape (a `count()`/`sum(…)`/… projection) lowers to a
    // `GROUP BY`/`HAVING` SELECT — no pagination or keyset; the row filter (query
    // `where` + soft-delete + `@scope`) still narrows rows *before* grouping.
    if let Some(shape) = rq
        .ret_shape
        .as_deref()
        .and_then(|n| find_shape(decls, n, &rq.target))
    {
        if shape_has_agg(&shape.body) {
            return lower_agg_query(q, root, sel, &shape.body, dialect);
        }
    }

    // 1. Projection (drives the SELECT list; also seeds joins for reached columns).
    let mut projection = build_projection(&mut sel, decls, rq, root);

    // 2. Filter conditions: the query's own predicate first, then injected guards.
    let mut wheres: Vec<String> = Vec::new();
    collect_filter(&mut sel, q, root, &mut wheres);
    if let Some(sd) = &root.soft_delete {
        wheres.push(soft_pred(dialect, &sel.root_alias, root, sd));
    }
    // `@scope` rides into every query on the model unless the query opts out
    // with `unscoped(...)`. The injected predicate is the *chosen alternative* — the
    // axes this query named — resolved by sema per callable.
    if let Some(scope) = sel.scope_where(&sel.root_alias, root) {
        wheres.push(scope);
    }

    // 3. Sort cascade + keyset tiebreaker. The resolved keys (their `table`.`col`
    //    references + directions) drive both the ORDER BY and the keyset comparison.
    let order_keys = build_order(&mut sel, q, root);
    let order: Vec<String> = order_keys
        .iter()
        .map(|k| order_by_term(&k.col_ref, k.dir, k.nullable, k.nulls, sel.dialect))
        .collect();

    // 4. Keyset pagination: a `page` without `offset` compares against
    //    an opaque cursor. The runtime binds the cursor's sort-key values into
    //    `:keyset_<i>` and flips `:keyset_active`; here we emit the lexicographic
    //    "strictly after the cursor" predicate plus the hidden `__keyset_<i>` columns
    //    the runtime reads to mint the next cursor. The count query stays cursor-free
    //    (it is the page-independent live-row total), so the guard lands on the main
    //    WHERE only. Requires resolved sort keys (the tiebreaker guarantees ≥1).
    let keyset = query_page(q)
        .filter(|p| !p.offset && !order_keys.is_empty())
        .map(|_| order_keys.iter().map(|k| k.prim).collect::<Vec<_>>());
    let mut main_wheres = wheres.clone();
    if keyset.is_some() {
        let hidden = order_keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                format!(
                    "  {} AS {}",
                    k.col_ref,
                    sel.q(&format!("{KEYSET_PREFIX}{i}"))
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        projection = format!("{projection},\n{hidden}");
        main_wheres.push(format!(
            "(:keyset_active = 0 OR ({}))",
            keyset_predicate(&order_keys, sel.dialect)
        ));
    }

    // Assemble. Joins were accumulated by every resolve above, so emit them now.
    let distinct = if query_distinct(q) { " DISTINCT" } else { "" };
    let mut sql = format!("SELECT{distinct}\n{}\nFROM {}", projection, sel.qt(root));
    push_joins(&mut sql, sel.dialect, &sel.joins);
    if !main_wheres.is_empty() {
        sql.push_str(&format!("\nWHERE {}", main_wheres.join(" AND ")));
    }
    if !order.is_empty() {
        sql.push_str(&format!("\nORDER BY {}", order.join(", ")));
    }
    if let Some(page) = query_page(q) {
        sql.push_str(&format!("\nLIMIT {}", page.size));
        if page.offset {
            sql.push_str(" OFFSET :offset");
        }
    }
    push_lock_clause(&mut sql, q, sel.dialect); // `for update` row lock, appended last
    sql.push_str(";\n");

    // `with count`: a second query for the live-row total (soft-delete applied, no
    // LIMIT). Meaningless for keyset, hence opt-in.
    let count_sql = query_page(q)
        .filter(|p| p.with_count)
        .map(|_| count_query(&sel, root, &projection, &wheres, query_distinct(q)));

    LoweredQuery {
        name: q.name.node.clone(),
        sql,
        count_sql,
        keyset,
        json_paths: json_output_paths(schema, decls, rq.ret_shape.as_deref(), &rq.target, root),
    }
}

/// The `with count` companion query: the live-row total (soft-delete/scope applied, no
/// LIMIT). Under `distinct` the total must count the **deduped** projected rows, not the
/// raw joined rows — so it wraps the `SELECT DISTINCT <projection>` in a derived table
/// (a bare `COUNT(*)` would overcount and break the client's page-count math). The plain
/// path counts rows directly.
fn count_query(
    sel: &Select,
    root: &RModel,
    projection: &str,
    wheres: &[String],
    distinct: bool,
) -> String {
    let mut cnt = if distinct {
        let mut inner = format!("SELECT DISTINCT\n{projection}\nFROM {}", sel.qt(root));
        push_joins(&mut inner, sel.dialect, &sel.joins);
        if !wheres.is_empty() {
            inner.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
        }
        format!(
            "SELECT COUNT(*) AS {}\nFROM ({inner}) AS {}",
            sel.q("count"),
            sel.q("distinct_rows")
        )
    } else {
        let mut cnt = format!(
            "SELECT COUNT(*) AS {}\nFROM {}",
            sel.q("count"),
            sel.qt(root)
        );
        push_joins(&mut cnt, sel.dialect, &sel.joins);
        if !wheres.is_empty() {
            cnt.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
        }
        cnt
    };
    cnt.push_str(";\n");
    cnt
}
