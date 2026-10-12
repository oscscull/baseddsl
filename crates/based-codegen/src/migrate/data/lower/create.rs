//! Lower a data-step `create` to one multi-row `INSERT`, with its upsert tail.

use based_ast::{DataAssign, DataConflict, DataRow};

use super::super::super::unit::IdKind;
use super::super::check::DataError;
use super::super::table::TableView;
use super::Lw;
use crate::Dialect;

impl Lw<'_> {
    pub(crate) fn create(
        &self,
        rows: &[DataRow],
        conflict: Option<&DataConflict>,
        t: &TableView,
    ) -> Result<(String, Vec<IdKind>), DataError> {
        let authored: Vec<&str> = rows
            .first()
            .map(|r| r.assigns.iter().map(|a| a.col.node.as_str()).collect())
            .unwrap_or_default();
        let minted = t.minted_pk().filter(|(pk, _)| !authored.contains(pk));
        let tuples = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let vals = authored
                    .iter()
                    .map(|c| match row.assigns.iter().find(|a| a.col.node == *c) {
                        Some(a) => self.expr(&a.value, t, t.col(c)),
                        None => Ok("NULL".to_string()),
                    })
                    .chain(minted.map(|_| Ok(placeholder(self.dialect, i + 1))))
                    .collect::<Result<Vec<_>, DataError>>()?;
                Ok(format!("({})", vals.join(", ")))
            })
            .collect::<Result<Vec<_>, DataError>>()?;
        let cols: Vec<String> = authored
            .iter()
            .copied()
            .chain(minted.map(|(pk, _)| pk))
            .map(|c| self.q(c))
            .collect();
        let tail = conflict
            .map(|c| self.conflict_tail(c, &authored, t))
            .transpose()?
            .unwrap_or_default();
        let ids = minted.map_or_else(Vec::new, |(_, kind)| vec![kind; rows.len()]);
        Ok((
            format!(
                "INSERT INTO {} ({}) VALUES {}{tail}",
                self.qt(t),
                cols.join(", "),
                tuples.join(", ")
            ),
            ids,
        ))
    }

    /// ` ON CONFLICT (…) DO UPDATE SET …` / ` ON DUPLICATE KEY UPDATE …`.
    fn conflict_tail<'a>(
        &self,
        c: &'a DataConflict,
        inserted: &[&'a str],
        t: &TableView,
    ) -> Result<String, DataError> {
        let set_lw = Lw {
            qualify: None,
            incoming: true,
            writes_root: false,
            ..self.nested(String::new())
        };
        let target: Vec<&str> = c.target.iter().map(|i| i.node.as_str()).collect();
        let assign = |a: &'a DataAssign| {
            (
                a.col.node.as_str(),
                set_lw.expr(&a.value, t, t.col(&a.col.node)),
            )
        };
        // `...incoming` sits at a lexical position: assigns before it are overridden by
        // it, assigns after it override it.
        let split = c.spread.as_ref().map_or(c.update.len(), |s| s.preceding);
        let spread = c.spread.iter().flat_map(|_| {
            inserted
                .iter()
                .filter(|col| !target.contains(col))
                .map(|col| (*col, Ok(set_lw.incoming_ref(col))))
        });
        let sets = c.update[..split]
            .iter()
            .map(assign)
            .chain(spread)
            .chain(c.update[split..].iter().map(assign))
            .try_fold(Vec::new(), |mut sets: Vec<(&str, String)>, (col, sql)| {
                let sql = sql?;
                sets.retain(|(c, _)| *c != col);
                sets.push((col, sql));
                Ok(sets)
            })?;
        let assigns = sets
            .iter()
            .map(|(c, v)| format!("{} = {v}", self.q(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let keys = target
            .iter()
            .map(|k| self.q(k))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(match (self.dialect.is_mysql_family(), assigns.is_empty()) {
            (true, true) => {
                let k = self.q(target[0]);
                format!(" ON DUPLICATE KEY UPDATE {k} = {k}")
            }
            (true, false) => format!(" ON DUPLICATE KEY UPDATE {assigns}"),
            (false, true) => format!(" ON CONFLICT ({keys}) DO NOTHING"),
            (false, false) => format!(" ON CONFLICT ({keys}) DO UPDATE SET {assigns}"),
        })
    }
}

/// The `n`th (1-based) bind placeholder.
fn placeholder(dialect: Dialect, n: usize) -> String {
    match dialect {
        Dialect::Postgres => format!("${n}"),
        Dialect::MariaDb | Dialect::MySql | Dialect::Sqlite => "?".to_string(),
    }
}
