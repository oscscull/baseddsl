use super::*;

/// The `SET` clause left-hand side for a column. MySQL/MariaDB accept (and this code
/// emits) a table-qualified `` `t`.`col` ``, which a multi-table UPDATE's `SET` may need to
/// disambiguate the target. **Postgres forbids the target alias in `SET`**, and **SQLite
/// rejects a qualified column in an UPDATE `SET`** (it has no inline-join UPDATE, so the
/// target is always unambiguous) — both take the bare column (`col = …`), the alias
/// belonging only to the `FROM`/`WHERE`. So this qualifies on MySQL/MariaDB and stays bare
/// on Postgres + SQLite.
pub(crate) fn set_lhs(sel: &Select, _model: &RModel, col: &str) -> String {
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
pub(crate) fn update_stmt(sel: &Select, model: &RModel, sets: &[String], wheres: &[String]) -> String {
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
pub(crate) fn delete_stmt(sel: &Select, model: &RModel, wheres: &[String]) -> String {
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

pub(crate) fn push_where(s: &mut String, wheres: &[String]) {
    if !wheres.is_empty() {
        s.push_str(&format!("\nWHERE {}", wheres.join(" AND ")));
    }
}

/// Append the soft-delete live predicate (when `live`) and the callable's chosen
/// `@scope` to a write's `WHERE`, so a mutation can't touch a tombstoned or
/// out-of-scope row. An `unscoped` callable injects no scope (`scope_where` returns
/// `None` — its `scope_inject` is empty); soft-delete still applies (a separate
/// guarantee).
pub(crate) fn inject_guards(sel: &mut Select, model: &RModel, wheres: &mut Vec<String>, live: bool) {
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
pub(crate) fn updated_bump(sel: &Select, model: &RModel, assigned: &[String]) -> Option<String> {
    let field = model.updated.as_deref()?;
    let col = physical_col(model, field);
    if assigned.contains(&col) {
        return None;
    }
    Some(format!("{} = CURRENT_TIMESTAMP", set_lhs(sel, model, &col)))
}

/// The `SET` fragment that writes (or clears) the tombstone for the covered subset
/// timestamp `CURRENT_TIMESTAMP`/`NULL`, bool `TRUE`/`FALSE`.
pub(crate) fn tombstone_set(sel: &Select, model: &RModel, sd: &SoftDelete, deleting: bool) -> String {
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
pub(crate) fn timestamp_cols(model: &RModel, fields: &[Option<&str>]) -> Vec<String> {
    let mut cols: Vec<String> = Vec::new();
    for f in fields.iter().flatten() {
        let col = physical_col(model, f);
        if !cols.contains(&col) {
            cols.push(col);
        }
    }
    cols
}
