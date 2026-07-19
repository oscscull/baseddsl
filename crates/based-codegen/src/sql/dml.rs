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

use std::collections::HashMap;

use based_ast::*;
use based_sema::{
    CheckedSchema, EnumValue, MemberKind, REnum, RModel, RQuery, ScopeInject, SoftDelete, SoftMode,
};

use crate::Dialect;

/// The separator joining a nested to-one relation's field name to its projected
/// columns in a SELECT output alias (`buyer` + `name` → `buyer.name`). A `.` cannot
/// occur in a BSL identifier, so any output alias containing it is unambiguously a
/// nested projection — the runtime (`run.rs`) splits on it to reassemble the flat row
/// into a sub-object. One source of truth for the convention: codegen
/// emits it, the runtime reads it.
pub const NEST_SEP: char = '.';

/// The output-alias suffix marking a to-**many** nested array (`items { … }` → alias
/// `items[]`). The column's value is a JSON-array *string* (per-dialect JSON aggregation,
/// [`crate::Dialect::json_array_agg`]) of the nested sub-objects; the runtime (`run.rs`)
/// sees the `[]` suffix, parses the string into a real JSON array, and stores it under
/// the field name without the suffix. `[`/`]` cannot occur in a BSL identifier, so the
/// marker never collides with a projected field. One source of the convention: codegen
/// emits it, the runtime reads it. Composes with [`NEST_SEP`] — a to-many
/// inside a to-one nests as `parent.items[]`.
pub const ARRAY_MARK: &str = "[]";

/// The output-alias prefix for a keyset query's hidden cursor-basis columns. Each
/// sort key `k_i` is projected an extra time as `<k_i> AS __keyset_<i>` so the runtime
/// can read the last row's sort-key values to mint the next cursor, then strip these
/// columns from the response. The `__` prefix cannot begin a BSL identifier, so it can
/// never collide with a projected field. One source of the convention: codegen
/// emits it, the runtime (`run.rs`) reads + strips it.
pub const KEYSET_PREFIX: &str = "__keyset_";

/// Presence probe for a to-one nest whose row may be absent (a LEFT-JOINed edge:
/// an optional forward relation, or a to-one inverse). The child's `id` is projected
/// once more as `<field>.__present`; the runtime collapses the nested object to JSON
/// `null` when it is NULL (an all-null sub-object would otherwise be indistinguishable
/// from a matched row) and strips the probe. `__` cannot begin a BSL identifier.
pub const NEST_PRESENT: &str = "__present";

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
    let mut out = Vec::new();
    for decl in decls {
        if let Decl::Query(q) = decl {
            if let Some(rq) = queries.get(q.name.node.as_str()) {
                out.push(lower_query(schema, decls, q, rq, dialect));
            }
        }
    }
    out
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
        };
    }
    // `unscoped` opts the whole query out of scope handling — the joined
    // tables' `@scope` as well as the root's, kept in one decision.
    let mut sel = Select::new(schema, decls, root, dialect)
        .with_scope_inject(q.unscoped.is_none())
        .with_scope_terms(&rq.scope_inject);

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
        .map(|k| format!("{} {}", k.col_ref, dir(k.dir)))
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
            keyset_predicate(&order_keys)
        ));
    }

    // Assemble. Joins were accumulated by every resolve above, so emit them now.
    let mut sql = format!("SELECT\n{}\nFROM {}", projection, sel.q(&root.table));
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
    sql.push_str(";\n");

    // `with count`: a second query for the live-row total (soft-delete applied, no
    // LIMIT). Meaningless for keyset, hence opt-in.
    let count_sql = if query_page(q).is_some_and(|p| p.with_count) {
        let mut cnt = format!(
            "SELECT COUNT(*) AS {}\nFROM {}",
            sel.q("count"),
            sel.q(&root.table)
        );
        push_joins(&mut cnt, sel.dialect, &sel.joins);
        if !wheres.is_empty() {
            cnt.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
        }
        cnt.push_str(";\n");
        Some(cnt)
    } else {
        None
    };

    LoweredQuery {
        name: q.name.node.clone(),
        sql,
        count_sql,
        keyset,
    }
}

/// The keyset "strictly after the cursor" predicate over the ordered sort keys
/// For keys `k0 dir0, k1 dir1, …` and cursor values `:keyset_0, …`,
/// the row-comparison expands lexicographically:
/// `(k0 ▷ v0) OR (k0 = v0 AND k1 ▷ v1) OR …`, where `▷` is `>` for an ASC key and `<`
/// for a DESC key. The expanded form (rather than a `(k0,k1) > (v0,v1)` row-value
/// comparison) is used because SQL row comparison cannot mix ASC/DESC directions and
/// the expansion is portable across all three dialects. The final key is always the
/// unique `id` tiebreaker, so the comparison never drops or repeats a row.
fn keyset_predicate(keys: &[OrderKey]) -> String {
    (0..keys.len())
        .map(|i| {
            let mut ands: Vec<String> = (0..i)
                .map(|j| format!("{} = :keyset_{j}", keys[j].col_ref))
                .collect();
            let cmp = match keys[i].dir {
                SortDir::Asc => ">",
                SortDir::Desc => "<",
            };
            ands.push(format!("{} {} :keyset_{i}", keys[i].col_ref, cmp));
            format!("({})", ands.join(" AND "))
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

// ---------- projection -----------------------------------------------------

/// Build the indented SELECT-list text for a query. Delegates to [`project_return`],
/// the shape-projection core the write side reuses for its post-write re-select.
fn build_projection<'a>(
    sel: &mut Select<'a>,
    decls: &'a [Decl],
    rq: &RQuery,
    root: &'a RModel,
) -> String {
    project_return(sel, decls, rq.ret_shape.as_deref(), &rq.target, root)
}

/// Build the indented SELECT-list text from a return type. Uses the named return shape
/// when present, else projects every stored column of a bare-model return. Shared by
/// the read side (queries) and the write side (`mutations`'s declared-shape re-select),
/// so a mutation returns the *same* projection a `get` of that shape would.
pub(crate) fn project_return<'a>(
    sel: &mut Select<'a>,
    decls: &'a [Decl],
    ret_shape: Option<&str>,
    target: &str,
    root: &'a RModel,
) -> String {
    let mut cols: Vec<String> = Vec::new();
    match ret_shape {
        Some(name) => {
            if let Some(shape) = find_shape(decls, name, target) {
                let root_alias = sel.root_alias.clone();
                project_body(sel, &shape.body, root, &root_alias, "", "", &mut cols);
            }
        }
        None => {
            // Bare-model return: every stored column, aliased to its field name.
            for mem in &root.members {
                match &mem.kind {
                    MemberKind::Scalar { column, .. } => {
                        cols.push(format!(
                            "{} AS {}",
                            sel.qcol(&sel.root_alias, column),
                            sel.q(&mem.name)
                        ));
                    }
                    MemberKind::Forward { fk_col, .. } => {
                        cols.push(format!(
                            "{} AS {}",
                            sel.qcol(&sel.root_alias, fk_col),
                            sel.q(&mem.name)
                        ));
                    }
                    MemberKind::Inverse { .. } => {}
                }
            }
        }
    }
    cols.iter()
        .map(|c| format!("  {c}"))
        .collect::<Vec<_>>()
        .join(",\n")
}

/// Project a shape body against `model`, appending `expr AS out` lines to `cols`.
///
/// `alias`/`prefix` locate the model in the join graph (the root alias + empty prefix
/// at the top; a joined alias + its path prefix inside a nest), so paths resolve from
/// the right table. `out_prefix` is prepended to every emitted output alias — empty at
/// the top, `field.` (one [`NEST_SEP`] per level) inside a nest — so a nested column
/// lands under a `parent.child` alias the runtime reassembles into a sub-object.
fn project_body<'a>(
    sel: &mut Select<'a>,
    fields: &'a [ShapeField],
    model: &'a RModel,
    alias: &str,
    prefix: &str,
    out_prefix: &str,
    cols: &mut Vec<String>,
) {
    for f in fields {
        match f {
            ShapeField::Bare(id) => {
                let (a, col) = sel.resolve_from(&single(&id.node), alias, prefix, model);
                cols.push(format!(
                    "{} AS {}",
                    sel.qcol(&a, &col),
                    sel.q(&out_alias(out_prefix, &id.node))
                ));
            }
            ShapeField::Rename { out, value } => match value {
                ShapeValue::Path(p) => {
                    let (a, col) = sel.resolve_from(p, alias, prefix, model);
                    cols.push(format!(
                        "{} AS {}",
                        sel.qcol(&a, &col),
                        sel.q(&out_alias(out_prefix, &out.node))
                    ));
                }
                ShapeValue::Raw(raw) => {
                    cols.push(format!(
                        "({}) AS {}",
                        render_raw(sel.dialect, raw, alias, &model.table),
                        sel.q(&out_alias(out_prefix, &out.node))
                    ));
                }
            },
            // A to-**one** relation nests the target's columns under a `field.`-prefixed
            // alias (reassembled by the runtime). A to-**many** relation aggregates the
            // child rows into a single JSON-array column (`field[]`) via a correlated
            // subquery, parsed back into an array by the runtime.
            ShapeField::Nest { field, body } => {
                project_nest(sel, field, body, model, alias, prefix, out_prefix, cols);
            }
            // `field -> Shape`: same lowering as an inline nest, the body coming from
            // the named shape's decl. Sema rejects reference cycles; the stack guard
            // keeps this terminating on an unchecked schema.
            ShapeField::NestRef { field, shape } => {
                let Some(body) = sel.enter_shape_ref(&shape.node) else {
                    continue;
                };
                project_nest(sel, field, body, model, alias, prefix, out_prefix, cols);
                sel.exit_shape_ref();
            }
        }
    }
}

/// Lower one relation nest (`field { body }`, or a `field -> Shape` expansion): a
/// to-one edge projects the child's columns under a `field.`-prefixed alias; a
/// to-many edge aggregates the child rows into a JSON-array column (`field[]`).
#[allow(clippy::too_many_arguments)]
fn project_nest<'a>(
    sel: &mut Select<'a>,
    field: &Ident,
    body: &'a [ShapeField],
    model: &'a RModel,
    alias: &str,
    prefix: &str,
    out_prefix: &str,
    cols: &mut Vec<String>,
) {
    if let Some((child_alias, child_prefix, child_model)) =
        sel.enter_to_one(&field.node, alias, prefix, model)
    {
        let nested_out = format!("{out_prefix}{}{NEST_SEP}", field.node);
        if to_one_absent_possible(model, &field.node) {
            cols.push(format!(
                "{} AS {}",
                sel.qcol(&child_alias, "id"),
                sel.q(&format!("{nested_out}{NEST_PRESENT}"))
            ));
        }
        project_body(
            sel,
            body,
            child_model,
            &child_alias,
            &child_prefix,
            &nested_out,
            cols,
        );
    } else if let Some((child_model, via_fk, edge_sort)) = sel.to_many_edge(&field.node, model) {
        let arr = sel.json_array_subquery(body, child_model, &via_fk, alias, edge_sort);
        let out = out_alias(out_prefix, &format!("{}{ARRAY_MARK}", field.node));
        cols.push(format!("{arr} AS {}", sel.q(&out)));
    }
}

/// Whether a to-one nest's joined row can be absent: an optional forward relation
/// or a to-one inverse — the LEFT-JOINed edges. A required forward edge inner-joins,
/// so its row always exists. Mirrors the client emitter's `Option<…>` typing.
fn to_one_absent_possible(model: &RModel, field: &str) -> bool {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Forward { optional, .. }) => *optional,
        Some(MemberKind::Inverse { .. }) => true,
        _ => false,
    }
}

/// Prepend the nest output prefix to a field's output alias (`""` at the top,
/// `"buyer."` inside a nest → `"buyer.name"`).
fn out_alias(prefix: &str, name: &str) -> String {
    format!("{prefix}{name}")
}

// ---------- filters (the query's own predicate) ---------------------------

/// Append the query's filter conditions. Bare/inline queries map each param to a
/// same-name equality (or its per-param binding); block/inline queries also carry
/// explicit `where` clauses referencing params via `$`.
fn collect_filter(sel: &mut Select, q: &Query, root: &RModel, out: &mut Vec<String>) {
    let is_block = matches!(q.body, QueryBody::Block(_) | QueryBody::Raw(_));
    if !is_block {
        for p in &q.params {
            out.push(param_condition(sel, p, root));
        }
    }
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    for c in clauses {
        if let Clause::Where(pred) = c {
            out.push(sel.predicate(pred, root));
        }
    }
}

/// One bare/inline param -> a filter condition (per-param bindings).
fn param_condition(sel: &mut Select, p: &Param, root: &RModel) -> String {
    let ph = format!(":{}", p.name.node);
    match &p.binding {
        // `user -> author`: equality on the named relation's FK column.
        Some(ParamBinding::Edge(edge)) => {
            let (alias, col) = sel.resolve(&single(&edge.node), root);
            format!("{} = {ph}", sel.qcol(&alias, &col))
        }
        // `since: timestamp > created_at`: explicit column + operator. The collection
        // ops mirror the predicate lowering — `in` takes a value list, `has` is JSON
        // containment (Postgres `col @> value`, MySQL-family `value MEMBER OF(col)`).
        Some(ParamBinding::ColOp { op, col }) => {
            let (alias, c) = sel.resolve(&single(&col.node), root);
            let lhs = sel.qcol(&alias, &c);
            match op {
                Op::In => format!("{lhs} IN ({ph})"),
                Op::Has => match sel.dialect {
                    Dialect::Postgres => format!("{lhs} @> {ph}"),
                    _ => format!("{ph} MEMBER OF({lhs})"),
                },
                _ => format!("{lhs} {} {ph}", sql_op(*op)),
            }
        }
        // same-name equality on the mapped column (a relation field maps to its FK).
        None => {
            let (alias, col) = sel.resolve(&single(&p.name.node), root);
            format!("{} = {ph}", sel.qcol(&alias, &col))
        }
    }
}

// ---------- sort cascade ---------------------------------------------------

/// One resolved sort key: its quoted `table`.`col` reference, direction, and the
/// column's primitive (the type the runtime re-binds the cursor value as). Drives both
/// the ORDER BY and (for a keyset page) the cursor comparison, so the two can't drift.
struct OrderKey {
    col_ref: String,
    dir: SortDir,
    prim: Primitive,
}

/// query `order (...)` > model `@sort` > none (sema already lints the empty case).
/// Keyset queries (paginated, not `offset`) append `id` as a unique tiebreaker.
fn build_order(sel: &mut Select, q: &Query, root: &RModel) -> Vec<OrderKey> {
    let query_order: Option<&[SortTerm]> = match &q.body {
        QueryBody::Inline(cs) => cs.iter().find_map(order_of),
        QueryBody::Block(s) => s.clauses.iter().find_map(order_of),
        QueryBody::Bare | QueryBody::Raw(_) => None,
    };
    let terms: &[SortTerm] = query_order.unwrap_or(&root.sort);

    let mut out: Vec<OrderKey> = Vec::new();
    let mut last_is_id = false;
    for t in terms {
        let prim = path_primitive(sel.schema, root, &t.path);
        let (alias, col) = sel.resolve(&t.path, root);
        last_is_id = alias == sel.root_alias && col == "id";
        out.push(OrderKey {
            col_ref: sel.qcol(&alias, &col),
            dir: t.dir,
            prim,
        });
    }
    if let Some(page) = query_page(q) {
        // A keyset page must be deterministic: append the unique `id`
        // tiebreaker unless the sort already ends on it. This holds even with no
        // explicit `order`/`@sort` — an empty order still yields `ORDER BY id`, so the
        // cursor comparison has a unique basis and never drops or repeats a row. Offset
        // pages don't need the tiebreaker (their window is positional).
        if !page.offset && !last_is_id {
            // The tiebreaker's primitive is the model's own `id` type: a declared
            // `id: text` cursor value must re-bind as text, not uuid.
            let prim = match root.member("id").map(|m| &m.kind) {
                Some(MemberKind::Scalar { ty, .. }) => *ty,
                _ => Primitive::Id,
            };
            out.push(OrderKey {
                col_ref: sel.qcol(&sel.root_alias, "id"),
                dir: SortDir::Asc,
                prim,
            });
        }
    }
    out
}

/// The primitive a dotted sort path terminates in, walked against the schema: a scalar
/// is its own primitive, a relation terminal is the FK it sorts by (a uuid). An
/// unresolved path (sema already flagged) falls back to `Text` so lowering terminates.
fn path_primitive(schema: &CheckedSchema, root: &RModel, path: &Path) -> Primitive {
    let mut cur = root;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind) {
            Some(MemberKind::Scalar { ty, .. }) => return *ty,
            Some(MemberKind::Forward { target, .. }) | Some(MemberKind::Inverse { target, .. }) => {
                if last {
                    return Primitive::Uuid;
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return Primitive::Text,
                }
            }
            None => return Primitive::Text,
        }
    }
    Primitive::Text
}

// ---------- the join-accumulating resolver --------------------------------

/// A pending JOIN. Deduped by `alias` (one per traversed path prefix), so a shape
/// and a `where` that both reach through `placed_by` share the single join.
pub(crate) struct Join {
    pub(crate) kind: &'static str, // "JOIN" | "LEFT JOIN"
    pub(crate) table: String,
    pub(crate) alias: String,
    pub(crate) on: String,
}

/// Append each accumulated join to a statement (`<kind> <table> AS <alias> ON <on>`),
/// identifiers quoted for the dialect. Shared by the read side (SELECT/COUNT) and the
/// write side (`mutations`'s multi-table UPDATE/DELETE) so the two can't drift.
pub(crate) fn push_joins(s: &mut String, dialect: Dialect, joins: &[Join]) {
    for j in joins {
        s.push_str(&format!(
            "\n{} {} AS {} ON {}",
            j.kind,
            dialect.quote(&j.table),
            dialect.quote(&j.alias),
            j.on
        ));
    }
}

/// Accumulates joins as paths are resolved, so the final FROM/JOIN block reflects
/// every column any clause reached across. Shared by the read side (this module)
/// and the write side (`mutations`), which reuses `predicate`/`value` so a
/// mutation `where` lowers identically to a query `where`.
pub(crate) struct Select<'a> {
    schema: &'a CheckedSchema,
    /// The compile target. Drives identifier quoting (`` `x` `` vs `"x"`) and a few
    /// operator/literal spellings; everything else is portable across dialects.
    pub(crate) dialect: Dialect,
    pub(crate) root_alias: String,
    pub(crate) joins: Vec<Join>,
    /// path-prefix key (e.g. "placed_by", "address.city") -> join alias.
    seen: HashMap<String, String>,
    /// Named filters by name, so a `FilterCall` (or a bare atom naming a filter) can
    /// inline its body against the call-site model — the codegen mirror of the sema check.
    filters: HashMap<&'a str, &'a NamedFilter>,
    /// Filters currently mid-expansion; guards a self-referential filter from looping
    /// (sema permits recursion, so we must terminate on our own, like sema does).
    filter_stack: Vec<&'a str>,
    /// Named shapes by name (`full` excluded — it is per-model and never referenced by
    /// name), so a `field -> Shape` nest can expand the referenced body in place.
    shapes: HashMap<&'a str, &'a Shape>,
    /// Shape references currently mid-expansion; sema rejects reference cycles
    /// (`E0134`), so this only keeps codegen terminating on an unchecked schema.
    shape_stack: Vec<&'a str>,
    /// The immediately preceding `create` in an enclosing `tx`, so a `^.field`
    /// back-reference can bind to it. `None` outside a `tx`.
    back: Option<BackCtx<'a>>,
    /// Whether to inject a *joined* scoped model's `@scope` into its join `ON`.
    /// True by default; set false for an `unscoped` callable, which opts out of
    /// *all* scope handling — the joined tables included, not just the root. The
    /// root/write-target `@scope` is injected by the caller (`lower_query` /
    /// `lower_write`), which already honours `unscoped`; this flag governs only the
    /// join-`ON` injection the resolver performs as it materializes each join.
    inject_scope: bool,
    /// The per-touched-model scope the *current callable* injects, from
    /// `RQuery`/`RMutation.scope_inject`. Keyed by model name; each entry is the
    /// chosen alternative's `(column, ctx_field)` terms. The root `WHERE`, the joined
    /// `ON`, and the create auto-set all read the terms for their model from here, so a
    /// callable naming one alternative injects a different predicate than one naming
    /// another. Empty for an `unscoped` callable (sema returns no injection).
    scope_inject: &'a [ScopeInject],
    /// Monotonic counter minting a distinct root alias (`s<n>_<table>`) for each to-many
    /// nested-array subquery, so a self-referential edge's child table never collides
    /// with the outer row's alias. Threaded through nested subqueries so siblings stay
    /// unique.
    sub_counter: usize,
}

/// What a `^.field` back-reference resolves to: the preceding `create`'s bound `id`
/// parameter and its assigns (to reuse a caller-supplied value for a non-`id` field).
#[derive(Clone)]
pub(crate) struct BackCtx<'a> {
    /// The bind name the prior create's app-generated `id` was emitted under
    /// (`id_<step>` inside a tx). `^.id` lowers to this.
    pub(crate) id_param: String,
    pub(crate) assigns: &'a [Assign],
}

impl<'a> Select<'a> {
    pub(crate) fn new(
        schema: &'a CheckedSchema,
        decls: &'a [Decl],
        root: &RModel,
        dialect: Dialect,
    ) -> Self {
        let filters = decls
            .iter()
            .filter_map(|d| match d {
                Decl::Filter(f) => Some((f.name.node.as_str(), f)),
                _ => None,
            })
            .collect();
        let shapes = decls
            .iter()
            .filter_map(|d| match d {
                Decl::Shape(s) if s.name.node != "full" => Some((s.name.node.as_str(), s)),
                _ => None,
            })
            .collect();
        Select {
            schema,
            dialect,
            root_alias: root.table.clone(),
            joins: Vec::new(),
            seen: HashMap::new(),
            filters,
            filter_stack: Vec::new(),
            shapes,
            shape_stack: Vec::new(),
            back: None,
            inject_scope: true,
            scope_inject: &[],
            sub_counter: 0,
        }
    }

    /// Enter a `field -> Shape` expansion: the referenced shape's body, or `None` for
    /// an unknown name or a reference already mid-expansion (a cycle sema rejects).
    /// Every `Some` must be paired with an [`exit_shape_ref`](Self::exit_shape_ref).
    fn enter_shape_ref(&mut self, name: &str) -> Option<&'a [ShapeField]> {
        if self.shape_stack.contains(&name) {
            return None;
        }
        let shape = self.shapes.get(name).copied()?;
        self.shape_stack.push(shape.name.node.as_str());
        Some(&shape.body)
    }

    fn exit_shape_ref(&mut self) {
        self.shape_stack.pop();
    }

    /// Quote one identifier for the target dialect (`` `x` `` / `"x"`).
    pub(crate) fn q(&self, ident: &str) -> String {
        self.dialect.quote(ident)
    }

    /// A `table`.`column` qualified reference, quoted for the dialect.
    pub(crate) fn qcol(&self, table: &str, column: &str) -> String {
        self.dialect.qcol(table, column)
    }

    /// Attach a tx back-reference context so a `^.field` in this statement's assigns
    /// binds to the preceding `create`.
    pub(crate) fn with_back(mut self, back: Option<BackCtx<'a>>) -> Self {
        self.back = back;
        self
    }

    /// Set whether a joined scoped model's `@scope` rides into its join `ON`.
    /// An `unscoped` callable passes `false` to drop scope from the joined tables
    /// too — the same opt-out the root/write-target scope already honours.
    pub(crate) fn with_scope_inject(mut self, inject: bool) -> Self {
        self.inject_scope = inject;
        self
    }

    /// Attach the current callable's per-model chosen scope injection, from
    /// `RQuery`/`RMutation.scope_inject`. Every scope predicate this `Select` emits —
    /// root `WHERE`, joined `ON`, create auto-set — reads its terms from here.
    pub(crate) fn with_scope_terms(mut self, inject: &'a [ScopeInject]) -> Self {
        self.scope_inject = inject;
        self
    }

    /// The scope `(column, ctx_field)` terms the current callable injects for `model`
    /// (the alternative it chose), or `&[]` when the callable confines this model
    /// by no scope (unscoped, or the model isn't touched).
    pub(crate) fn scope_terms_for(&self, model: &str) -> &[(String, String)] {
        self.scope_inject
            .iter()
            .find(|si| si.model == model)
            .map(|si| si.terms.as_slice())
            .unwrap_or(&[])
    }

    /// The `@scope` conjunction the current callable injects for `model`, anchored at
    /// `alias`: each chosen term `col = $ctx.field` becomes `<alias>.<col> =
    /// :ctx_<field>`, ANDed. `None` when the callable confines this model by no scope
    /// (unscoped callable, or a model carrying none of the named axes). The bind name
    /// `:ctx_<field>` is the *same* one every scope site uses, so the runtime binds it
    /// once from the request `$ctx`. For a single-alternative model this is the
    /// model's whole scope.
    pub(crate) fn scope_where(&self, alias: &str, model: &RModel) -> Option<String> {
        let terms: Vec<String> = self
            .scope_terms_for(&model.name)
            .iter()
            .map(|(field, ctx_field)| {
                format!(
                    "{} = :ctx_{ctx_field}",
                    self.qcol(alias, &physical_col(model, field))
                )
            })
            .collect();
        (!terms.is_empty()).then(|| terms.join(" AND "))
    }

    /// The joined-`ON` scope injection: the callable's chosen `@scope` for the joined
    /// `model`, or `None` when scope injection is off (`unscoped` callable) — the
    /// join-`ON` twin of the root `scope_where`.
    fn scope_join_pred(&self, alias: &str, model: &RModel) -> Option<String> {
        if !self.inject_scope {
            return None;
        }
        self.scope_where(alias, model)
    }

    /// Resolve a dotted path from `root` to `(table_alias, column)`, materializing
    /// a JOIN for each relation step. A terminal relation resolves to its FK column
    /// (so `where (org = $org)` compares `org_id`), never a join.
    pub(crate) fn resolve(&mut self, path: &Path, root: &RModel) -> (String, String) {
        let root_alias = self.root_alias.clone();
        self.resolve_from(path, &root_alias, "", root)
    }

    /// Materialize the JOIN for a to-**one** relation member `field` on `model` (rooted
    /// at `alias`/`prefix`), returning `(joined_alias, joined_prefix, target_model)` for
    /// projecting the nested sub-object's columns. `None` for a scalar (not a relation)
    /// or a to-**many** inverse edge (handled by the to-many subquery path, not here).
    /// A Forward relation is always to-one; an Inverse is
    /// to-one only when its paired forward FK (`via`) is unique on the target (a genuine
    /// one-to-one back edge), else it is a collection.
    pub(crate) fn enter_to_one(
        &mut self,
        field: &str,
        alias: &str,
        prefix: &str,
        model: &RModel,
    ) -> Option<(String, String, &'a RModel)> {
        let mem = model.member(field)?;
        let mut prefix = prefix.to_string();
        match &mem.kind {
            MemberKind::Forward {
                target,
                fk_col,
                optional,
                ..
            } => {
                let (a, m) =
                    self.join_forward(alias, &mut prefix, field, target, fk_col, *optional);
                Some((a, prefix, m))
            }
            MemberKind::Inverse { target, via } => {
                let tmodel = self.schema.model(target)?;
                if !tmodel.is_unique(via) {
                    return None; // to-many collection — handled by the to-many subquery path.
                }
                let (a, m) = self.join_inverse(alias, &mut prefix, field, target, via);
                Some((a, prefix, m))
            }
            MemberKind::Scalar { .. } => None,
        }
    }

    /// A to-**many** relation edge `field` on `model` (an Inverse whose paired forward FK
    /// is *not* unique — a genuine collection), as `(child_model, back_fk_column,
    /// edge_sort)`. `None` for a scalar, a forward relation, or a to-one inverse (those
    /// are `enter_to_one`'s). The back FK is the column on the child carrying the
    /// relation back to `model` (`OrderItem.order` → `order_id`), used to correlate the
    /// aggregating subquery. `edge_sort` is the edge's own relation `@sort` (empty when
    /// undeclared) — the traversal tier of the sort cascade ordering the nested array.
    pub(crate) fn to_many_edge(
        &self,
        field: &str,
        model: &'a RModel,
    ) -> Option<(&'a RModel, String, &'a [SortTerm])> {
        let member = model.member(field)?;
        match &member.kind {
            MemberKind::Inverse { target, via } => {
                let tmodel = self.schema.model(target)?;
                if tmodel.is_unique(via) {
                    return None; // to-one back edge — handled by `enter_to_one`.
                }
                let via_fk = match tmodel.member(via).map(|m| &m.kind) {
                    Some(MemberKind::Forward { fk_col, .. }) => fk_col.clone(),
                    _ => format!("{via}_id"),
                };
                Some((tmodel, via_fk, &member.sort))
            }
            _ => None,
        }
    }

    /// Build the correlated-subquery expression that aggregates a to-many child edge into
    /// a JSON array of the projected element bodies (L1). The child gets a fresh
    /// `s<n>_<table>` root alias (distinct from `outer_alias`, so a self-referential edge
    /// works), the element body is projected as a per-dialect JSON object, and the
    /// subquery correlates the child's back FK to `outer_alias.id`, applying the child's
    /// soft-delete tombstone + `@scope` exactly as a join would. The array's element
    /// order follows the sort cascade for the traversal — the edge's relation `@sort`
    /// (`edge_sort`), else the child model's `@sort` — as an ORDER BY *inside* the
    /// aggregate; with neither declared the order stays unspecified.
    fn json_array_subquery(
        &mut self,
        body: &'a [ShapeField],
        child: &'a RModel,
        via_fk: &str,
        outer_alias: &str,
        edge_sort: &[SortTerm],
    ) -> String {
        self.sub_counter += 1;
        let child_alias = format!("s{}_{}", self.sub_counter, child.table);
        // Fresh join scope for the subquery: its reaches/to-one nests accumulate joins
        // into `sub`, not the outer SELECT. The counter is threaded through so nested
        // subqueries keep minting distinct aliases.
        let mut sub = Select {
            schema: self.schema,
            dialect: self.dialect,
            root_alias: child_alias.clone(),
            joins: Vec::new(),
            seen: HashMap::new(),
            filters: self.filters.clone(),
            filter_stack: Vec::new(),
            shapes: self.shapes.clone(),
            // Shape refs mid-expansion carry across the subquery boundary, so a
            // reference cycle spanning a to-many nest still terminates.
            shape_stack: self.shape_stack.clone(),
            back: None,
            inject_scope: self.inject_scope,
            scope_inject: self.scope_inject,
            sub_counter: self.sub_counter,
        };
        let elem = sub.json_object_expr(body, child, &child_alias, "");
        // Sort cascade for the traversal: relation `@sort` on the edge beats the child
        // model's `@sort`. Terms resolve against the child (dotted paths join inside
        // the subquery's own scope).
        let sort_terms: &[SortTerm] = if edge_sort.is_empty() {
            &child.sort
        } else {
            edge_sort
        };
        let order_keys: Vec<String> = sort_terms
            .iter()
            .map(|t| {
                let (a, col) = sub.resolve_from(&t.path, &child_alias, "", child);
                format!("{} {}", sub.qcol(&a, &col), dir(t.dir))
            })
            .collect();
        let order = (!order_keys.is_empty()).then(|| order_keys.join(", "));
        self.sub_counter = sub.sub_counter;

        let mut wheres = vec![format!(
            "{} = {}",
            self.qcol(&child_alias, via_fk),
            self.qcol(outer_alias, "id")
        )];
        if let Some(sd) = &child.soft_delete {
            wheres.push(soft_pred(self.dialect, &child_alias, child, sd));
        }
        if let Some(scope) = sub.scope_join_pred(&child_alias, child) {
            wheres.push(scope);
        }

        let mut sql = format!(
            "(SELECT {} FROM {} AS {}",
            self.dialect.json_array_agg(&elem, order.as_deref()),
            self.q(&child.table),
            self.q(&child_alias)
        );
        push_joins(&mut sql, self.dialect, &sub.joins);
        sql.push_str(&format!(" WHERE {})", wheres.join(" AND ")));
        sql
    }

    /// Build a per-dialect JSON-object expression (`json_object('k', v, …)`) for a shape
    /// body over `model` at `alias`/`prefix` — one element of a to-many nested array. A
    /// bare field / reach becomes a `'key', <col>` pair; a to-one nest a nested JSON
    /// object; a to-many nest a nested correlated-subquery array. Reaches and to-one nests
    /// materialize their joins into this (sub-)`Select`'s join scope.
    fn json_object_expr(
        &mut self,
        body: &'a [ShapeField],
        model: &'a RModel,
        alias: &str,
        prefix: &str,
    ) -> String {
        let mut pairs: Vec<String> = Vec::new();
        for f in body {
            match f {
                ShapeField::Bare(id) => {
                    let path = single(&id.node);
                    let (a, col) = self.resolve_from(&path, alias, prefix, model);
                    let expr = self.json_scalar(&a, &col, &path, model);
                    pairs.push(format!("'{}', {expr}", id.node));
                }
                ShapeField::Rename { out, value } => match value {
                    ShapeValue::Path(p) => {
                        let (a, col) = self.resolve_from(p, alias, prefix, model);
                        let expr = self.json_scalar(&a, &col, p, model);
                        pairs.push(format!("'{}', {expr}", out.node));
                    }
                    ShapeValue::Raw(raw) => {
                        pairs.push(format!(
                            "'{}', ({})",
                            out.node,
                            render_raw(self.dialect, raw, alias, &model.table)
                        ));
                    }
                },
                ShapeField::Nest { field, body } => {
                    self.json_nest_pair(field, body, model, alias, prefix, &mut pairs);
                }
                ShapeField::NestRef { field, shape } => {
                    if let Some(body) = self.enter_shape_ref(&shape.node) {
                        self.json_nest_pair(field, body, model, alias, prefix, &mut pairs);
                        self.exit_shape_ref();
                    }
                }
            }
        }
        format!("{}({})", self.dialect.json_object_fn(), pairs.join(", "))
    }

    /// One scalar column inside a JSON element body. A `decimal` column is cast to
    /// text first: the wire contract carries a decimal as its exact JSON *string*, but
    /// a SQL-built JSON object would render the native numeric as a JSON number and
    /// lose the contract (SQLite already stores decimal as TEXT — no cast needed).
    fn json_scalar(&self, alias: &str, col: &str, path: &Path, model: &RModel) -> String {
        let qcol = self.qcol(alias, col);
        if !matches!(
            path_primitive(self.schema, model, path),
            Primitive::Decimal { .. }
        ) {
            return qcol;
        }
        match self.dialect {
            Dialect::Postgres => format!("({qcol})::text"),
            Dialect::MariaDb => format!("CAST({qcol} AS CHAR)"),
            Dialect::Sqlite => qcol,
        }
    }

    /// One relation nest inside a JSON element body: a to-one edge becomes a nested
    /// JSON object, a to-many edge a nested correlated-subquery array.
    fn json_nest_pair(
        &mut self,
        field: &Ident,
        body: &'a [ShapeField],
        model: &'a RModel,
        alias: &str,
        prefix: &str,
        pairs: &mut Vec<String>,
    ) {
        if let Some((child_alias, child_prefix, child_model)) =
            self.enter_to_one(&field.node, alias, prefix, model)
        {
            let nested = self.json_object_expr(body, child_model, &child_alias, &child_prefix);
            // An absent LEFT-JOINed row must surface as JSON null, not an object of
            // nulls — probe the child's `id` (never NULL on a matched row).
            let nested = if to_one_absent_possible(model, &field.node) {
                format!(
                    "CASE WHEN {} IS NULL THEN NULL ELSE {nested} END",
                    self.qcol(&child_alias, "id")
                )
            } else {
                nested
            };
            pairs.push(format!("'{}', {}", field.node, nested));
        } else if let Some((child_model, via_fk, edge_sort)) = self.to_many_edge(&field.node, model)
        {
            let arr = self.json_array_subquery(body, child_model, &via_fk, alias, edge_sort);
            pairs.push(format!("'{}', {}", field.node, arr));
        }
    }

    /// Resolve a dotted path starting from `start_model` (aliased `start_alias`, at join
    /// path `start_prefix`) to `(table_alias, column)`, materializing a JOIN per relation
    /// step. [`resolve`](Self::resolve) is this rooted at the query's root; a nested shape
    /// body resolves its paths from the joined relation's alias/prefix instead.
    pub(crate) fn resolve_from(
        &mut self,
        path: &Path,
        start_alias: &str,
        start_prefix: &str,
        start_model: &RModel,
    ) -> (String, String) {
        let mut cur = start_model;
        let mut alias = start_alias.to_string();
        let mut prefix = start_prefix.to_string();
        let n = path.segments.len();
        for (i, seg) in path.segments.iter().enumerate() {
            let name = &seg.node;
            let mem = match cur.member(name) {
                Some(m) => m,
                None => return (alias, name.clone()), // sema already flagged this
            };
            let last = i + 1 == n;
            match &mem.kind {
                MemberKind::Scalar { column, .. } => return (alias, column.clone()),
                MemberKind::Forward {
                    target,
                    fk_col,
                    optional,
                    ..
                } => {
                    if last {
                        return (alias, fk_col.clone());
                    }
                    let (next_alias, next) =
                        self.join_forward(&alias, &mut prefix, name, target, fk_col, *optional);
                    alias = next_alias;
                    cur = next;
                }
                MemberKind::Inverse { target, via } => {
                    if last {
                        // Equality against a to-many edge has no local column; fall
                        // back to the key (unusual; kept resolvable).
                        return (alias, "id".to_string());
                    }
                    let (next_alias, next) =
                        self.join_inverse(&alias, &mut prefix, name, target, via);
                    alias = next_alias;
                    cur = next;
                }
            }
        }
        (alias, "id".to_string())
    }

    /// FK on this table -> JOIN target ON target.id = cur.fk. Optional -> LEFT JOIN.
    fn join_forward(
        &mut self,
        cur_alias: &str,
        prefix: &mut String,
        field: &str,
        target: &str,
        fk_col: &str,
        optional: bool,
    ) -> (String, &'a RModel) {
        push_prefix(prefix, field);
        let tmodel = self.schema.model(target).expect("relation target resolved");
        if let Some(a) = self.seen.get(prefix) {
            return (a.clone(), tmodel);
        }
        let alias = format!("j_{}", prefix.replace('.', "_"));
        let kind = if optional { "LEFT JOIN" } else { "JOIN" };
        let mut on = format!(
            "{} = {}",
            self.qcol(&alias, "id"),
            self.qcol(cur_alias, fk_col)
        );
        if let Some(sd) = &tmodel.soft_delete {
            on.push_str(&format!(
                " AND {}",
                soft_pred(self.dialect, &alias, tmodel, sd)
            ));
        }
        // A joined *scoped* model rides its `@scope` into the `ON` too — the
        // exact parallel of the soft-delete injection above, so a query reaching
        // another tenant's row through a relation can't read across the scope
        // boundary. A LEFT JOIN stays a left join (the predicate is in `ON`, not
        // `WHERE`): an out-of-scope joined row simply yields NULLs.
        if let Some(scope) = self.scope_join_pred(&alias, tmodel) {
            on.push_str(&format!(" AND {scope}"));
        }
        self.record(kind, tmodel.table.clone(), alias.clone(), on, prefix);
        (alias, tmodel)
    }

    /// FK on the target table -> LEFT JOIN target ON target.<via_fk> = cur.id.
    fn join_inverse(
        &mut self,
        cur_alias: &str,
        prefix: &mut String,
        field: &str,
        target: &str,
        via: &str,
    ) -> (String, &'a RModel) {
        push_prefix(prefix, field);
        let tmodel = self.schema.model(target).expect("relation target resolved");
        if let Some(a) = self.seen.get(prefix) {
            return (a.clone(), tmodel);
        }
        let alias = format!("j_{}", prefix.replace('.', "_"));
        // The forward field `via` on the target carries the FK column back to us.
        let via_fk = match tmodel.member(via).map(|m| &m.kind) {
            Some(MemberKind::Forward { fk_col, .. }) => fk_col.clone(),
            _ => format!("{via}_id"),
        };
        let mut on = format!(
            "{} = {}",
            self.qcol(&alias, &via_fk),
            self.qcol(cur_alias, "id")
        );
        if let Some(sd) = &tmodel.soft_delete {
            on.push_str(&format!(
                " AND {}",
                soft_pred(self.dialect, &alias, tmodel, sd)
            ));
        }
        // Joined-model `@scope` rides the `ON` too, same as the forward join.
        if let Some(scope) = self.scope_join_pred(&alias, tmodel) {
            on.push_str(&format!(" AND {scope}"));
        }
        self.record("LEFT JOIN", tmodel.table.clone(), alias.clone(), on, prefix);
        (alias, tmodel)
    }

    fn record(
        &mut self,
        kind: &'static str,
        table: String,
        alias: String,
        on: String,
        prefix: &str,
    ) {
        self.seen.insert(prefix.to_string(), alias.clone());
        self.joins.push(Join {
            kind,
            table,
            alias,
            on,
        });
    }

    // ---------- predicate lowering (where / @scope) -----------------------

    pub(crate) fn predicate(&mut self, p: &Predicate, model: &RModel) -> String {
        match p {
            Predicate::And(a, b) => {
                format!(
                    "({} AND {})",
                    self.predicate(a, model),
                    self.predicate(b, model)
                )
            }
            Predicate::Or(a, b) => {
                format!(
                    "({} OR {})",
                    self.predicate(a, model),
                    self.predicate(b, model)
                )
            }
            Predicate::Not(inner) => format!("NOT ({})", self.predicate(inner, model)),
            Predicate::Cmp { path, op, value } => {
                let (alias, col) = self.resolve(path, model);
                let lhs = self.qcol(&alias, &col);
                // An enum column compares against a bare variant, which lowers to its
                // wire string literal (not a column reference).
                let rhs = self
                    .enum_variant_lit(model, path, value)
                    .unwrap_or_else(|| self.value(value, model));
                match op {
                    // Collection ops don't fit plain infix: `in` needs a value list,
                    // `has` is JSON-array containment — MySQL's `value MEMBER OF(arr)`
                    // vs. Postgres's `arr @> value` (the JSONB containment operator).
                    Op::In => format!("{lhs} IN ({rhs})"),
                    Op::Has => match self.dialect {
                        Dialect::Postgres => format!("{lhs} @> {rhs}"),
                        _ => format!("{rhs} MEMBER OF({lhs})"),
                    },
                    _ => format!("{lhs} {} {rhs}", sql_op(*op)),
                }
            }
            // `path in (v, v, …)`: each element lowers like an equality RHS — an
            // enum variant to its wire value, a `$param` to its own placeholder
            // (bound positionally), a literal per-dialect.
            Predicate::InList { path, values } => {
                let (alias, col) = self.resolve(path, model);
                let lhs = self.qcol(&alias, &col);
                let mut items = Vec::with_capacity(values.len());
                for v in values {
                    let item = self
                        .enum_variant_lit(model, path, v)
                        .unwrap_or_else(|| self.value(v, model));
                    items.push(item);
                }
                format!("{lhs} IN ({})", items.join(", "))
            }
            // A bare atom is either a zero-arg named filter or a plain bool column.
            Predicate::Bare(path) => {
                if path.segments.len() == 1 {
                    if let Some(f) = self.filters.get(path.segments[0].node.as_str()).copied() {
                        return self.filter_call(f, &[], model);
                    }
                }
                let (alias, col) = self.resolve(path, model);
                format!(
                    "{} = {}",
                    self.qcol(&alias, &col),
                    self.dialect.bool_lit(true)
                )
            }
            // Inline the filter's body, substituting args for its params, resolved
            // against the call-site model (sema guarantees the body resolves).
            Predicate::FilterCall { name, args } => match self.filters.get(name.node.as_str()) {
                Some(f) => self.filter_call(f, args, model),
                None => format!("TRUE /* filter {} unresolved */", name.node),
            },
            Predicate::Raw(raw) => format!(
                "({})",
                render_raw(self.dialect, raw, &self.root_alias, &model.table)
            ),
        }
    }

    /// The enum a dotted path terminates on, when the terminal column is enum-typed
    /// (read-only, no join materialized). Lets the caller render a variant RHS as its
    /// wire value.
    fn terminal_enum(&self, path: &Path, model: &RModel) -> Option<&REnum> {
        let mut cur = model;
        let n = path.segments.len();
        for (i, seg) in path.segments.iter().enumerate() {
            let mem = cur.member(&seg.node)?;
            let last = i + 1 == n;
            match &mem.kind {
                MemberKind::Scalar {
                    enum_name: Some(name),
                    ..
                } if last => return self.schema.enum_(name),
                MemberKind::Scalar { .. } => return None,
                MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                    if last {
                        return None;
                    }
                    cur = self.schema.model(target)?;
                }
            }
        }
        None
    }

    /// If `path` names an enum column and `value` is a bare single-segment variant,
    /// its wire value literal (`'paid'` or `2`); else `None` to fall back to value lowering.
    fn enum_variant_lit(&self, model: &RModel, path: &Path, value: &Value) -> Option<String> {
        let en = self.terminal_enum(path, model)?;
        variant_lit(self.dialect, en, value)
    }

    /// If assigning enum column `col_field` a bare single-segment variant, its wire value
    /// literal; else `None`.
    pub(crate) fn enum_assign_lit(
        &self,
        model: &RModel,
        col_field: &str,
        value: &Value,
    ) -> Option<String> {
        match model.member(col_field).map(|m| &m.kind) {
            Some(MemberKind::Scalar {
                enum_name: Some(name),
                ..
            }) => {
                let en = self.schema.enum_(name)?;
                variant_lit(self.dialect, en, value)
            }
            _ => None,
        }
    }

    pub(crate) fn value(&mut self, v: &Value, model: &RModel) -> String {
        match v {
            Value::Param(pr) => format!(":{}", param_key(pr)),
            Value::Path(p) => {
                let (alias, col) = self.resolve(p, model);
                self.qcol(&alias, &col)
            }
            Value::Lit(l) => render_lit(self.dialect, l),
            Value::Func(f) => render_func(f),
            Value::Back(b) => self.back_value(b),
        }
    }

    /// Lower a `^.field` back-reference. `^.id` binds to the preceding
    /// create's app-generated id (`:id_<step>`); any other field reuses the value the
    /// prior create assigned to it (a caller param/literal), which the engine already
    /// binds. Sema (E0170) guarantees a prior create and a real field exist.
    fn back_value(&self, b: &BackRef) -> String {
        let Some(back) = &self.back else {
            return "NULL /* ^ needs a prior create */".to_string();
        };
        // Reuse the value the prior create assigned to this field (a caller
        // param/literal the engine already binds), if it set one.
        if let Some(a) = back.assigns.iter().find(|a| a.col.node == b.field.node) {
            return match &a.value {
                Value::Param(pr) => format!(":{}", param_key(pr)),
                Value::Lit(l) => render_lit(self.dialect, l),
                Value::Func(f) => render_func(f),
                // A path or nested back-ref in the prior create is not a plain bind;
                // leave a visible marker rather than emit something unbindable.
                _ => format!("NULL /* ^.{} unresolved */", b.field.node),
            };
        }
        // Otherwise: `^.id` is the app-generated id the prior create binds under
        // `:id_<step>`; any other unset field needs a re-select (runtime).
        if b.field.node == "id" {
            format!(":{}", back.id_param)
        } else {
            format!("NULL /* ^.{} not set by prior create */", b.field.node)
        }
    }

    /// Inline a named filter: bind its params to the call arguments, substitute those
    /// bindings through its body, then lower the result against `model`. The filter
    /// carries no model of its own, so its column paths resolve at the call site.
    fn filter_call(&mut self, f: &'a NamedFilter, args: &[Value], model: &RModel) -> String {
        // Recursion guard: a self-referential filter is legal (sema terminates it);
        // stop re-expanding and leave a visible marker rather than looping.
        if self.filter_stack.contains(&f.name.node.as_str()) {
            return format!("TRUE /* filter {} recursion */", f.name.node);
        }
        // Arity is enforced by sema (E0115); guard defensively against a mismatch.
        if f.params.len() != args.len() {
            return format!("TRUE /* filter {} arity */", f.name.node);
        }
        let binds: HashMap<&str, &Value> = f
            .params
            .iter()
            .map(|p| p.name.node.as_str())
            .zip(args)
            .collect();
        let body = subst_pred(&f.pred, &binds);
        self.filter_stack.push(f.name.node.as_str());
        let sql = self.predicate(&body, model);
        self.filter_stack.pop();
        format!("({sql})")
    }
}

/// Substitute a filter's param bindings into its body. A filter param appears only
/// in value position (`= $c`, or an argument to a nested filter), so only `$name`
/// refs are rewritten; column paths are left to resolve against the call-site model.
fn subst_pred(p: &Predicate, binds: &HashMap<&str, &Value>) -> Predicate {
    match p {
        Predicate::And(a, b) => Predicate::And(
            Box::new(subst_pred(a, binds)),
            Box::new(subst_pred(b, binds)),
        ),
        Predicate::Or(a, b) => Predicate::Or(
            Box::new(subst_pred(a, binds)),
            Box::new(subst_pred(b, binds)),
        ),
        Predicate::Not(inner) => Predicate::Not(Box::new(subst_pred(inner, binds))),
        Predicate::Cmp { path, op, value } => Predicate::Cmp {
            path: path.clone(),
            op: *op,
            value: subst_value(value, binds),
        },
        Predicate::InList { path, values } => Predicate::InList {
            path: path.clone(),
            values: values.iter().map(|v| subst_value(v, binds)).collect(),
        },
        Predicate::Bare(path) => Predicate::Bare(path.clone()),
        Predicate::FilterCall { name, args } => Predicate::FilterCall {
            name: name.clone(),
            args: args.iter().map(|a| subst_value(a, binds)).collect(),
        },
        Predicate::Raw(raw) => Predicate::Raw(raw.clone()),
    }
}

/// Replace a bare `$name` value with its bound argument. A `$name.path` or an
/// unbound `$name` (e.g. `$ctx`) is left untouched; nested function args recurse.
fn subst_value(v: &Value, binds: &HashMap<&str, &Value>) -> Value {
    match v {
        Value::Param(pr) if pr.path.is_empty() => match binds.get(pr.name.node.as_str()) {
            Some(rep) => (*rep).clone(),
            None => v.clone(),
        },
        Value::Func(f) => Value::Func(FuncCall {
            name: f.name.clone(),
            args: f.args.iter().map(|a| subst_value(a, binds)).collect(),
        }),
        _ => v.clone(),
    }
}

// ---------- small helpers --------------------------------------------------

/// Soft-delete predicate for a table alias.
pub(crate) fn soft_pred(dialect: Dialect, alias: &str, model: &RModel, sd: &SoftDelete) -> String {
    let col = column_of(model, &sd.field);
    match sd.mode {
        SoftMode::Timestamp => format!("{} IS NULL", dialect.qcol(alias, &col)),
        SoftMode::Bool => format!(
            "{} = {}",
            dialect.qcol(alias, &col),
            dialect.bool_lit(false)
        ),
    }
}

/// Physical column backing a scalar field (its `(column …)` override or its name).
fn column_of(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        _ => field.to_string(),
    }
}

/// Physical column backing any field: a scalar's column, or a forward relation's FK
/// (`<field>_id`). Falls back to the field name (inverse edge / unknown — the latter
/// sema already rejected). Used by the write side to map `field = $x` assignments.
pub(crate) fn physical_col(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        Some(MemberKind::Forward { fk_col, .. }) => fk_col.clone(),
        _ => field.to_string(),
    }
}

/// A one-segment path, for the many call sites that resolve a single field name.
pub(crate) fn single(name: &str) -> Path {
    Path {
        segments: vec![Spanned {
            node: name.to_string(),
            span: NO_SPAN,
        }],
    }
}

const NO_SPAN: Span = Span {
    file: FileId(0),
    start: 0,
    end: 0,
};

fn push_prefix(prefix: &mut String, seg: &str) {
    if !prefix.is_empty() {
        prefix.push('.');
    }
    prefix.push_str(seg);
}

/// `$ctx.org` -> `ctx_org`; `$id` -> `id`. Placeholder-safe (dots removed).
pub(crate) fn param_key(pr: &ParamRef) -> String {
    let mut k = pr.name.node.clone();
    for seg in &pr.path {
        k.push('_');
        k.push_str(&seg.node);
    }
    k
}

fn order_of(c: &Clause) -> Option<&[SortTerm]> {
    match c {
        Clause::Order(terms) => Some(terms),
        _ => None,
    }
}

fn query_page(q: &Query) -> Option<&PageClause> {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => return None,
    };
    clauses.iter().find_map(|c| match c {
        Clause::Page(p) => Some(p),
        _ => None,
    })
}

/// Find the shape body for a return. `full` is per-model, so match on `from` too.
fn find_shape<'a>(decls: &'a [Decl], name: &str, model: &str) -> Option<&'a Shape> {
    decls.iter().find_map(|d| match d {
        Decl::Shape(s) if s.name.node == name && (name != "full" || s.from.node == model) => {
            Some(s)
        }
        _ => None,
    })
}

pub(crate) fn sql_op(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "<>",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "LIKE",
        Op::In => "IN",
        Op::Has => "MEMBER OF", // JSON array containment (MariaDB `x MEMBER OF(json)`)
    }
}

fn dir(d: SortDir) -> &'static str {
    match d {
        SortDir::Asc => "ASC",
        SortDir::Desc => "DESC",
    }
}

/// A bare single-segment variant value rendered as its wire value literal — a quoted
/// string for a string enum, a bare integer for an int enum — or `None` when `value` is
/// not a bare identifier (a `$param` binds normally; anything else falls through to
/// ordinary value lowering) or names no variant of `en`.
fn variant_lit(dialect: Dialect, en: &REnum, value: &Value) -> Option<String> {
    match value {
        Value::Path(vp) if vp.segments.len() == 1 => match en.wire_of(&vp.segments[0].node)? {
            EnumValue::Str(s) => Some(render_lit(dialect, &Literal::Str(s.clone()))),
            EnumValue::Int(n) => Some(n.to_string()),
        },
        _ => None,
    }
}

pub(crate) fn render_lit(dialect: Dialect, l: &Literal) -> String {
    match l {
        Literal::Str(s) => format!("'{}'", s.replace('\'', "''")),
        Literal::Int(i) => i.to_string(),
        Literal::Decimal(s) => s.clone(),
        Literal::Bool(b) => dialect.bool_lit(*b).to_string(),
        Literal::Null => "NULL".to_string(),
    }
}

pub(crate) fn render_func(f: &FuncCall) -> String {
    // `now()` is the only value-position function (ir::KNOWN_FUNCS).
    match f.name.node.as_str() {
        "now" => "CURRENT_TIMESTAMP".to_string(),
        other => format!("{other}()"),
    }
}

/// Render a raw-SQL fragment: text verbatim, `${param}` -> `:param`,
/// `{table}`/`{id}` -> safe engine interpolation (root table / its `id`). Only the
/// engine-interpolated identifiers are dialect-quoted; the raw text is the user's and
/// is emitted verbatim (an escape hatch — they own its portability).
pub(crate) fn render_raw(dialect: Dialect, raw: &RawSql, root_alias: &str, table: &str) -> String {
    let mut s = String::new();
    for part in &raw.parts {
        match part {
            RawPart::Text(t) => s.push_str(t),
            RawPart::Param(pr) => s.push_str(&format!(":{}", param_key(pr))),
            RawPart::Engine(id) => match id.node.as_str() {
                "table" => s.push_str(&dialect.quote(table)),
                "id" => s.push_str(&dialect.qcol(root_alias, "id")),
                other => s.push_str(&dialect.quote(other)),
            },
        }
    }
    s
}
