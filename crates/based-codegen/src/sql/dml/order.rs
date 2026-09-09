//! Ordering, keyset-cursor comparison, pagination, and row locking.

use super::*;

/// The keyset "strictly after the cursor" predicate over the ordered sort keys
/// For keys `k0 dir0, k1 dir1, …` and cursor values `:keyset_0, …`,
/// the row-comparison expands lexicographically:
/// `(k0 ▷ v0) OR (k0 = v0 AND k1 ▷ v1) OR …`, where `▷` is `>` for an ASC key and `<`
/// for a DESC key. The expanded form (rather than a `(k0,k1) > (v0,v1)` row-value
/// comparison) is used because SQL row comparison cannot mix ASC/DESC directions and
/// the expansion is portable across all three dialects. The final key is always the
/// unique `id` tiebreaker, so the comparison never drops or repeats a row.
///
/// A nullable sort key needs NULL-aware SQL, or a plain `col < :v` silently drops every
/// NULL-valued row from the walk (`NULL < v` is `NULL`, not true). Each dialect's own
/// default NULL sort position — NULL lowest on MariaDB/SQLite, highest on Postgres —
/// decides where such rows fall, and [`keyset_after`]/[`keyset_eq`] render a comparison
/// that matches it, so the ORDER BY (left at the dialect default) and the cursor predicate
/// agree and every row is returned exactly once.
pub(crate) fn keyset_predicate(keys: &[OrderKey], dialect: Dialect) -> String {
    (0..keys.len())
        .map(|i| {
            let mut ands: Vec<String> = (0..i).map(|j| keyset_eq(&keys[j], j, dialect)).collect();
            ands.push(keyset_after(&keys[i], i, dialect));
            format!("({})", ands.join(" AND "))
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// The equality prefix step for sort key `j` (`col = :keyset_j`). A nullable key uses the
/// dialect's null-safe equality so a NULL at this position still chains into the following
/// key's comparison instead of collapsing the whole conjunct to NULL.
fn keyset_eq(key: &OrderKey, j: usize, dialect: Dialect) -> String {
    let param = format!(":keyset_{j}");
    if !key.nullable {
        return format!("{} = {param}", key.col_ref);
    }
    match dialect {
        Dialect::Sqlite => format!("{} IS {param}", key.col_ref),
        Dialect::MariaDb | Dialect::MySql => format!("{} <=> {param}", key.col_ref),
        Dialect::Postgres => format!("{} IS NOT DISTINCT FROM {param}", key.col_ref),
    }
}

/// The "strictly after" step for sort key `i`. A non-nullable key is a plain `col ▷ :v`.
/// A nullable key expands to cover NULL on either side, using the key's direction and the
/// dialect's default NULL position so the predicate ranks NULLs exactly as the ORDER BY.
fn keyset_after(key: &OrderKey, i: usize, dialect: Dialect) -> String {
    let param = format!(":keyset_{i}");
    let cmp = match key.dir {
        SortDir::Asc => ">",
        SortDir::Desc => "<",
    };
    let col = &key.col_ref;
    if !key.nullable {
        return format!("{col} {cmp} {param}");
    }
    // Where NULLs fall in this key's direction: NULL is lowest on MariaDB/SQLite, highest
    // on Postgres, and the direction flips that. `nulls_first` = NULLs sort before the
    // non-NULL values in this key's own order.
    // An explicit `nulls first|last` on the key pins the placement; otherwise the cursor
    // follows the dialect default (NULL is lowest on MariaDB/MySQL/SQLite, highest on Postgres).
    let nulls_first = match key.nulls {
        Some(NullsPlacement::First) => true,
        Some(NullsPlacement::Last) => false,
        None => {
            let nulls_low = matches!(dialect, Dialect::MariaDb | Dialect::MySql | Dialect::Sqlite);
            (key.dir == SortDir::Asc) == nulls_low
        }
    };
    // Parenthesized as a unit: it is ANDed behind the preceding keys' equality prefix, so
    // its inner `OR` must not escape the conjunction.
    if nulls_first {
        // NULLs lead: a NULL cursor is passed only by non-NULL rows; a non-NULL cursor is
        // passed by rows that plainly compare after it (NULL rows precede it, excluded).
        format!("(({param} IS NULL AND {col} IS NOT NULL) OR ({param} IS NOT NULL AND {col} {cmp} {param}))")
    } else {
        // NULLs trail: a NULL cursor is the last position (nothing is after it here); a
        // non-NULL cursor is passed by later non-NULL rows and by the trailing NULL rows.
        format!("({param} IS NOT NULL AND ({col} {cmp} {param} OR {col} IS NULL))")
    }
}

// ---------- sort cascade ---------------------------------------------------

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

    // The primary-key column(s) + each part's own primitive — the deterministic keyset
    // tiebreaker. One entry for a surrogate/natural key; the full tuple, in key order, for a
    // composite `@key`.
    let pk_cols: Vec<(String, Primitive)> = root
        .pk_members()
        .into_iter()
        .map(|m| {
            let prim = match &m.kind {
                MemberKind::Scalar { ty, .. } => *ty,
                // A relation key part (a junction FK) mirrors its own target's key type.
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
        .collect();
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

/// `get|list … for update` — a pessimistic locking read (`SELECT … FOR UPDATE`, a no-op on
/// SQLite via the [`Dialect`](crate::Dialect) seam). Block-body only. Sema confines it to
/// well-defined single-row sets; the client confines it to transaction transports.
fn query_for_update(q: &Query) -> Option<LockWait> {
    match &q.body {
        QueryBody::Block(s) => s.for_update,
        _ => None,
    }
}

/// Append the `for update` row-locking clause (after ORDER BY/LIMIT) per dialect: `FOR UPDATE`
/// (plus its optional `NOWAIT`/`SKIP LOCKED` wait mode) on Postgres/MySQL/MariaDB, nothing on
/// SQLite (its transaction lock already serializes writers).
pub(crate) fn push_lock_clause(sql: &mut String, q: &Query, dialect: Dialect) {
    if let Some(wait) = query_for_update(q) {
        let lock = dialect.for_update_clause(wait);
        if !lock.is_empty() {
            sql.push_str(&format!("\n{lock}"));
        }
    }
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
