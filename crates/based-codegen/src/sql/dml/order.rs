//! ORDER BY derivation (the sort cascade + keyset PK tiebreaker) and pagination-clause
//! detection.

use super::*;

/// One resolved sort key: its quoted `table`.`col` reference, direction, the column's
/// primitive (the type the runtime re-binds the cursor value as), and whether the column
/// is nullable (the cursor comparison is NULL-aware for a nullable key). Drives both the
/// ORDER BY and (for a keyset page) the cursor comparison, so the two can't drift.
pub(crate) struct OrderKey {
    pub(crate) col_ref: String,
    pub(crate) dir: SortDir,
    pub(crate) prim: Primitive,
    pub(crate) nullable: bool,
    /// Explicit `nulls first|last` on this key; `None` = the dialect's default placement.
    pub(crate) nulls: Option<NullsPlacement>,
}

/// query `order (...)` > model `@sort` > none (sema already lints the empty case).
/// Keyset queries (paginated, not `offset`) append `id` as a unique tiebreaker.
pub(crate) fn build_order(sel: &mut Select, q: &Query, root: &RModel) -> Vec<OrderKey> {
    let query_order: Option<&[SortTerm]> = match &q.body {
        QueryBody::Inline(cs) => cs.iter().find_map(order_of),
        QueryBody::Block(s) => s.clauses.iter().find_map(order_of),
        QueryBody::Bare | QueryBody::Raw(_) => None,
    };
    // A `distinct` list takes only its explicit `order` — the model `@sort` default is
    // suppressed, since a non-projected default-sort column would break `SELECT DISTINCT`
    // (and defeat the dedup). Sema (E0312) guarantees any explicit order is projected.
    let terms: &[SortTerm] = match query_order {
        Some(o) => o,
        None if query_distinct(q) => &[],
        None => &root.sort,
    };

    let pk_cols = pk_keyset_cols(sel, root);
    let mut out: Vec<OrderKey> = Vec::new();
    let mut last_is_pk = false;
    for t in terms {
        let prim = path_primitive(sel.schema, root, &t.path);
        let (alias, col) = sel.resolve(&t.path, root);
        last_is_pk =
            alias == sel.root_alias && pk_cols.len() == 1 && pk_cols[0].0.as_str() == col.as_str();
        out.push(OrderKey {
            col_ref: sel.qcol(&alias, &col),
            dir: t.dir,
            prim,
            nullable: path_nullable(sel.schema, root, &t.path),
            nulls: t.nulls,
        });
    }
    if let Some(page) = query_page(q) {
        // A keyset page must be deterministic: append the unique primary-key tiebreaker
        // unless the sort already ends on it. This holds even with no explicit
        // `order`/`@sort` — an empty order still yields `ORDER BY <pk>`, so the cursor
        // comparison has a unique basis and never drops or repeats a row. Offset pages
        // don't need the tiebreaker (their window is positional). A keyless (`@no_id`) model
        // has no PK to append — sema (E0263) guarantees its declared sort already carries a
        // unique tiebreaker. A composite `@key` appends the full key tuple, in key order.
        // The primary key is non-nullable, so its tiebreaker keys need no NULL handling.
        if !page.offset && !last_is_pk && !query_distinct(q) {
            for (col, prim) in &pk_cols {
                out.push(OrderKey {
                    col_ref: sel.qcol(&sel.root_alias, col),
                    dir: SortDir::Asc,
                    prim: *prim,
                    nullable: false,
                    nulls: None,
                });
            }
        }
    }
    out
}

/// The primary-key column(s) + each part's own primitive — the deterministic keyset
/// tiebreaker. One entry for a surrogate/natural key; the full tuple, in key order, for a
/// composite `@key`. A relation key part (a junction FK) mirrors its own target's key type.
fn pk_keyset_cols(sel: &Select, root: &RModel) -> Vec<(String, Primitive)> {
    root.pk_members()
        .into_iter()
        .map(|m| {
            let prim = match &m.kind {
                MemberKind::Scalar { ty, .. } => *ty,
                MemberKind::Forward { target, .. } => sel
                    .schema
                    .model(target)
                    .and_then(RModel::pk_member)
                    .and_then(|t| match &t.kind {
                        MemberKind::Scalar { ty, .. } => Some(*ty),
                        _ => None,
                    })
                    .unwrap_or(Primitive::Uuid),
                MemberKind::Inverse { .. } => Primitive::Id,
            };
            (m.physical_col().to_string(), prim)
        })
        .collect()
}

fn order_of(c: &Clause) -> Option<&[SortTerm]> {
    match c {
        Clause::Order(terms) => Some(terms),
        _ => None,
    }
}

/// `list distinct <M>` — emit `SELECT DISTINCT` and suppress the automatic sort cascade
/// (an injected key column would defeat the row dedup). Block-body only.
pub(crate) fn query_distinct(q: &Query) -> bool {
    matches!(&q.body, QueryBody::Block(s) if s.distinct)
}

pub(crate) fn query_page(q: &Query) -> Option<&PageClause> {
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

fn dir(d: SortDir) -> &'static str {
    match d {
        SortDir::Asc => "ASC",
        SortDir::Desc => "DESC",
    }
}

/// One ORDER BY term. An explicit `nulls first|last` on a nullable key is realized per dialect:
/// Postgres/SQLite emit native `NULLS FIRST|LAST`; MySQL/MariaDB (which have no such clause)
/// prepend a `col IS NULL` term — `IS NULL` is 0 for non-NULL and 1 for NULL, so ASC trails the
/// NULLs (`DESC` leads them). Without an explicit placement the dialect default stands.
pub(crate) fn order_by_term(
    col_or_expr: &str,
    sort_dir: SortDir,
    nullable: bool,
    nulls: Option<NullsPlacement>,
    dialect: Dialect,
) -> String {
    let base = format!("{col_or_expr} {}", dir(sort_dir));
    let Some(placement) = nulls.filter(|_| nullable) else {
        return base;
    };
    match dialect {
        Dialect::Postgres | Dialect::Sqlite => {
            let nl = match placement {
                NullsPlacement::First => "NULLS FIRST",
                NullsPlacement::Last => "NULLS LAST",
            };
            format!("{base} {nl}")
        }
        Dialect::MariaDb | Dialect::MySql => {
            let null_dir = match placement {
                NullsPlacement::Last => "",
                NullsPlacement::First => " DESC",
            };
            format!("{col_or_expr} IS NULL{null_dir}, {base}")
        }
    }
}
