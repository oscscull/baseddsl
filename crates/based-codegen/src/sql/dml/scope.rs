//! `@scope` standing-filter injection into WHERE and joined ON.

use super::*;

impl<'a> Select<'a> {
    /// Set whether a joined scoped model's `@scope` rides into its join `ON`.
    /// An `unscoped` callable passes `false` to drop scope from the joined tables
    /// too — the same opt-out the root/write-target scope already honours.
    pub(crate) fn with_scope_inject(mut self, inject: bool) -> Self {
        self.inject_scope = inject;
        self
    }

    /// Attach the current callable's per-model chosen scope injection, from
    /// `RQuery`/`RMutation.scope_inject`. Every scope predicate this `Select` emits —
    /// root `WHERE`, joined `ON`, create auto-set — reads its terms from here.
    pub(crate) fn with_scope_terms(mut self, inject: &'a [ScopeInject]) -> Self {
        self.scope_inject = inject;
        self
    }

    /// The scope `(column, ctx_field)` terms the current callable injects for `model`
    /// (the alternative it chose), or `&[]` when the callable confines this model
    /// by no scope (unscoped, or the model isn't touched).
    pub(crate) fn scope_terms_for(&self, model: &str) -> &[(String, String)] {
        self.scope_inject
            .iter()
            .find(|si| si.model == model)
            .map_or(&[][..], |si| si.terms.as_slice())
    }

    /// The `@scope` conjunction the current callable injects for `model`, anchored at
    /// `alias`: each chosen term `col = $ctx.field` becomes `<alias>.<col> =
    /// :ctx_<field>`, ANDed. `None` when the callable confines this model by no scope
    /// (unscoped callable, or a model carrying none of the named axes). The bind name
    /// `:ctx_<field>` is the *same* one every scope site uses, so the runtime binds it
    /// once from the request `$ctx`. For a single-alternative model this is the
    /// model's whole scope.
    pub(crate) fn scope_where(&self, alias: &str, model: &RModel) -> Option<String> {
        let terms: Vec<String> = self
            .scope_terms_for(&model.name)
            .iter()
            .map(|(field, ctx_field)| {
                format!(
                    "{} = :ctx_{ctx_field}",
                    self.qcol(alias, &physical_col(model, field))
                )
            })
            .collect();
        (!terms.is_empty()).then(|| terms.join(" AND "))
    }

    /// The joined-`ON` scope injection: the callable's chosen `@scope` for the joined
    /// `model`, or `None` when scope injection is off (`unscoped` callable) — the
    /// join-`ON` twin of the root `scope_where`.
    pub(crate) fn scope_join_pred(&self, alias: &str, model: &RModel) -> Option<String> {
        if !self.inject_scope {
            return None;
        }
        self.scope_where(alias, model)
    }
}
