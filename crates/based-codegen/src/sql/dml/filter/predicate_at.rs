//! Lower a predicate against a nested shape's row rather than the query root.

use super::super::*;

impl<'a> Select<'a> {
    /// [`predicate`](Self::predicate) with its paths rooted at `alias` (join path
    /// `prefix`) over `model` — a CASE `when` or a filter projection inside a nest.
    pub(crate) fn predicate_at(
        &mut self,
        p: &Predicate,
        alias: &str,
        prefix: &str,
        model: &RModel,
    ) -> String {
        let saved_alias = std::mem::replace(&mut self.root_alias, alias.to_string());
        let saved_prefix = std::mem::replace(&mut self.root_prefix, prefix.to_string());
        let sql = self.predicate(p, model);
        self.root_alias = saved_alias;
        self.root_prefix = saved_prefix;
        sql
    }
}
