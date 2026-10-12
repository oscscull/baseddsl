//! Lower a data-step `update` / `delete`.

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
    ) -> Result<String, DataError> {
        let sets = assigns
            .iter()
            .map(|a| {
                let value = self.expr(&a.value, t, t.col(&a.col.node))?;
                Ok(format!("{} = {value}", self.q(&a.col.node)))
            })
            .collect::<Result<Vec<_>, DataError>>()?;
        Ok(format!(
            "UPDATE {} SET {}{}",
            self.qt(t),
            sets.join(", "),
            self.where_sql(where_, t)?
        ))
    }

    pub(crate) fn delete(
        &self,
        where_: Option<&Predicate>,
        t: &TableView,
    ) -> Result<String, DataError> {
        Ok(format!(
            "DELETE FROM {}{}",
            self.qt(t),
            self.where_sql(where_, t)?
        ))
    }

    /// ` WHERE <pred>`, or nothing for an unfiltered whole-table write.
    fn where_sql(&self, where_: Option<&Predicate>, t: &TableView) -> Result<String, DataError> {
        Ok(where_
            .map(|p| self.predicate(p, t))
            .transpose()?
            .map(|p| format!(" WHERE {p}"))
            .unwrap_or_default())
    }
}
