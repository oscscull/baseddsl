//! Lower a data-step `create` to one multi-row `INSERT`, with its upsert tail.

use based_ast::{DataConflict, DataRow};

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
        let mut cols: Vec<&str> = rows
            .first()
            .map(|r| r.assigns.iter().map(|a| a.col.node.as_str()).collect())
            .unwrap_or_default();
        let authored = cols.len();
        let minted = t.minted_pk().filter(|(pk, _)| !cols.contains(pk));
        let mut ids = Vec::new();
        let mut tuples = Vec::with_capacity(rows.len());
        for row in rows {
            let mut vals = Vec::with_capacity(cols.len() + 1);
            for c in &cols {
                let a = row.assigns.iter().find(|a| &a.col.node == c);
                vals.push(match a {
                    Some(a) => self.expr(&a.value, t, t.col(c))?,
                    None => "NULL".to_string(),
                });
            }
            if let Some((_, kind)) = minted {
                ids.push(kind);
                vals.push(placeholder(self.dialect, ids.len()));
            }
            tuples.push(format!("({})", vals.join(", ")));
        }
        if let Some((pk, _)) = minted {
            cols.push(pk);
        }
        let mut sql = format!(
            "INSERT INTO {} ({}) VALUES {}",
            self.qt(t),
            cols.iter()
                .map(|c| self.q(c))
                .collect::<Vec<_>>()
                .join(", "),
            tuples.join(", ")
        );
        if let Some(c) = conflict {
            sql.push_str(&self.conflict_tail(c, &cols[..authored], t)?);
        }
        Ok((sql, ids))
    }

    /// ` ON CONFLICT (…) DO UPDATE SET …` / ` ON DUPLICATE KEY UPDATE …`.
    fn conflict_tail(
        &self,
        c: &DataConflict,
        inserted: &[&str],
        t: &TableView,
    ) -> Result<String, DataError> {
        let set_lw = Lw {
            dialect: self.dialect,
            snap: self.snap,
            root: self.root.clone(),
            qualify: None,
            incoming: true,
            writes_root: false,
            sub: std::cell::Cell::new(self.sub.get()),
        };
        let target: Vec<&str> = c.target.iter().map(|i| i.node.as_str()).collect();
        // `...incoming` sits at a lexical position: assigns before it are overridden by
        // it, assigns after it override it.
        let mut sets: Vec<(String, String)> = Vec::new();
        let mut put = |col: &str, sql: String| {
            sets.retain(|(c, _)| c != col);
            sets.push((col.to_string(), sql));
        };
        let split = c.spread.as_ref().map_or(c.update.len(), |s| s.preceding);
        for a in &c.update[..split] {
            put(&a.col.node, set_lw.expr(&a.value, t, t.col(&a.col.node))?);
        }
        if c.spread.is_some() {
            for col in inserted.iter().filter(|c| !target.contains(c)) {
                put(col, set_lw.incoming_ref(col));
            }
        }
        for a in &c.update[split..] {
            put(&a.col.node, set_lw.expr(&a.value, t, t.col(&a.col.node))?);
        }
        let assigns: Vec<String> = sets
            .iter()
            .map(|(c, v)| format!("{} = {v}", self.q(c)))
            .collect();
        Ok(if self.dialect.is_mysql_family() {
            let assigns = if assigns.is_empty() {
                let k = self.q(target[0]);
                vec![format!("{k} = {k}")]
            } else {
                assigns
            };
            format!(" ON DUPLICATE KEY UPDATE {}", assigns.join(", "))
        } else {
            let keys = target
                .iter()
                .map(|k| self.q(k))
                .collect::<Vec<_>>()
                .join(", ");
            if assigns.is_empty() {
                format!(" ON CONFLICT ({keys}) DO NOTHING")
            } else {
                format!(" ON CONFLICT ({keys}) DO UPDATE SET {}", assigns.join(", "))
            }
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
