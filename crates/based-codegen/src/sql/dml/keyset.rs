//! The keyset-cursor "strictly after" comparison predicate over the ordered sort keys, and
//! its splicing into a query's projection + WHERE.

use super::*;

/// Splice a keyset page's cursor basis into the query: append the hidden `<key> AS
/// __keyset_<i>` cursor-basis columns to `projection` (the runtime reads the last row's
/// values to mint the next cursor) and push the `(:keyset_active = 0 OR <after>)` guard onto
/// `main_wheres` (a no-op on page 1). Returns the sort-key primitives (in order) for a keyset
/// page — the runtime re-binds each `:keyset_<i>` as its own primitive — or `None` for a
/// non-paginated / offset-paginated query. Requires resolved sort keys (the tiebreaker
/// guarantees ≥1).
pub(crate) fn splice_keyset(
    sel: &Select,
    order_keys: &[OrderKey],
    q: &Query,
    projection: &mut String,
    main_wheres: &mut Vec<String>,
) -> Option<Vec<Primitive>> {
    let keyset = query_page(q)
        .filter(|p| !p.offset && !order_keys.is_empty())
        .map(|_| order_keys.iter().map(|k| k.prim).collect::<Vec<_>>());
    if keyset.is_some() {
        let hidden = order_keys
            .iter()
            .enumerate()
            .map(|(i, k)| format!("  {} AS {}", k.col_ref, sel.q(&format!("{KEYSET_PREFIX}{i}"))))
            .collect::<Vec<_>>()
            .join(",\n");
        *projection = format!("{projection},\n{hidden}");
        main_wheres.push(format!(
            "(:keyset_active = 0 OR ({}))",
            keyset_predicate(order_keys, sel.dialect)
        ));
    }
    keyset
}

/// The keyset "strictly after the cursor" predicate over the ordered sort keys.
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
fn keyset_predicate(keys: &[OrderKey], dialect: Dialect) -> String {
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
