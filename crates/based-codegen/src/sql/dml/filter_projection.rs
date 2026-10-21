//! Lower a named-filter projection (`out = filter(args)`) to a per-row bool.

use super::*;

impl<'a> Select<'a> {
    /// `CASE WHEN <filter> THEN TRUE ELSE FALSE END` over the row at `alias`: a filter
    /// that is NULL for the row (a NULL column or argument) projects `false`, never NULL.
    pub(crate) fn filter_projection(
        &mut self,
        fp: &FilterProjection,
        alias: &str,
        prefix: &str,
        model: &RModel,
    ) -> String {
        let call = Predicate::FilterCall {
            name: fp.name.clone(),
            args: fp.args.clone(),
        };
        let cond = self.predicate_at(&call, alias, prefix, model);
        format!(
            "CASE WHEN {cond} THEN {} ELSE {} END",
            self.dialect.bool_lit(true),
            self.dialect.bool_lit(false)
        )
    }
}
