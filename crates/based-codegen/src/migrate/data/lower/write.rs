//! Lower a data-step `update` / `delete`, optionally narrowed by an extra key-range condition
//! (a chunk's batch).

use based_ast::{DataAssign, Predicate};

use super::super::check::DataError;
use super::super::table::TableView;
use super::Lw;

impl Lw<'_> {
    pub(crate) fn update(
        &self,
        where_: Option<&Predicate>,
        assigns: &[DataAssign],
        t: &TableView,
        range: Option<&str>,
    ) -> Result<String, DataError> {
        let sets = assigns
            .iter()
            .map(|a| {
                Ok(format!(
                    "{} = {}",
                    self.q(&a.col.node),
                    self.expr(&a.value, t, t.col(&a.col.node))?
                ))
            })
            .collect::<Result<Vec<_>, DataError>>()?;
        let mut sql = format!("UPDATE {} SET {}", self.qt(t), sets.join(", "));
        sql.push_str(&self.where_sql(where_, t, range)?);
        Ok(sql)
    }

    pub(crate) fn delete(
        &self,
        where_: Option<&Predicate>,
        t: &TableView,
        range: Option<&str>,
    ) -> Result<String, DataError> {
        Ok(format!(
            "DELETE FROM {}{}",
            self.qt(t),
            self.where_sql(where_, t, range)?
        ))
    }

    /// ` WHERE <pred> [AND <range>]`, or nothing for an unfiltered whole-table write.
    pub(crate) fn where_sql(
        &self,
        where_: Option<&Predicate>,
        t: &TableView,
        range: Option<&str>,
    ) -> Result<String, DataError> {
        let mut conds = Vec::new();
        if let Some(p) = where_ {
            conds.push(self.predicate(p, t)?);
        }
        conds.extend(range.map(str::to_string));
        Ok(if conds.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conds.join(" AND "))
        })
    }
}
