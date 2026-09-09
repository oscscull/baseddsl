//! Assemble a query into its full SELECT statement and package it as [`LoweredQuery`] — the
//! one lowering the runtime consumes and the text emitter (`render`) frames.

use super::*;

/// A query lowered to its structured SQL: the primary SELECT plus, for a
/// `with count` page, the live-row COUNT. Both carry the `:name` placeholders
/// verbatim — the runtime binds them; the text emitter frames them with
/// `-- query` headers (`render::dml`). This is the one lowering; the emitter and the
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

/// Index the schema's queries by name, for the decl-order lowering passes.
pub(crate) fn index_queries(schema: &CheckedSchema) -> HashMap<&str, &RQuery> {
    schema
        .queries
        .iter()
        .map(|q| (q.name.as_str(), q))
        .collect()
}

/// Lower every query in the schema to its structured SQL, in declaration order.
/// The in-process runtime consumes this directly (no serialized artifact).
pub fn lower_queries(schema: &CheckedSchema, decls: &[Decl], dialect: Dialect) -> Vec<LoweredQuery> {
    let queries = index_queries(schema);
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

/// The single query lowering: build the primary SELECT (and count SELECT) as header-free
/// SQL. Both `render::render_query` (text) and the runtime read this.
pub(crate) fn lower_query(
    schema: &CheckedSchema,
    decls: &[Decl],
    q: &Query,
    rq: &RQuery,
    dialect: Dialect,
) -> LoweredQuery {
    let root = schema.model(&rq.target).expect("target resolved by sema");

    // A whole-query raw body IS the statement — nothing engine-built composes.
    if let QueryBody::Raw(raw) = &q.body {
        return lower_raw_query(q, root, dialect, raw);
    }

    // `unscoped` opts the whole query out of scope handling — the joined
    // tables' `@scope` as well as the root's, kept in one decision.
    let mut sel = Select::new(schema, decls, root, dialect)
        .with_scope_inject(q.unscoped.is_none())
        .with_scope_terms(&rq.scope_inject);

    // An aggregate return shape (a `count()`/`sum(…)`/… projection) lowers to a
    // `GROUP BY`/`HAVING` SELECT — no pagination or keyset; the row filter still narrows
    // rows *before* grouping.
    if let Some(shape) = rq
        .ret_shape
        .as_deref()
        .and_then(|n| find_shape(decls, n, &rq.target))
    {
        if shape_has_agg(&shape.body) {
            return lower_agg_query(q, root, sel, &shape.body, dialect);
        }
    }

    // Projection (drives the SELECT list; also seeds joins for reached columns), then the
    // row filter, then the sort cascade + keyset tiebreaker.
    let mut projection = build_projection(&mut sel, decls, rq, root);
    let wheres = build_wheres(&mut sel, q, root, dialect);
    let order_keys = build_order(&mut sel, q, root);
    let order: Vec<String> = order_keys
        .iter()
        .map(|k| order_by_term(&k.col_ref, k.dir, k.nullable, k.nulls, sel.dialect))
        .collect();

    // Keyset pagination splices its hidden cursor columns into the projection and its
    // cursor guard onto the main WHERE (the count query stays cursor-free).
    let mut main_wheres = wheres.clone();
    let keyset = splice_keyset(&sel, &order_keys, q, &mut projection, &mut main_wheres);

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

/// A whole-query raw body: text verbatim, `${param}` → `:param` (bound like any
/// placeholder), `{table}`/`{id}` → the target model's table. Nothing engine-built
/// composes — no injected soft-delete/scope, no sort cascade, no pagination; the declared
/// shape types the result columns by name.
fn lower_raw_query(q: &Query, root: &RModel, dialect: Dialect, raw: &RawSql) -> LoweredQuery {
    let body = render_raw(dialect, raw, &root.table, &root.table);
    let body = body.trim_end().trim_end_matches(';').trim_end();
    LoweredQuery {
        name: q.name.node.clone(),
        sql: format!("{body};\n"),
        count_sql: None,
        keyset: None,
        json_paths: Vec::new(),
    }
}
