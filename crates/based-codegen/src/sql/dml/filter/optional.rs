//! Lower optional inputs: `?` optional-param present-guards (absent → widen to TRUE), and
//! null-safe `$ctx.field?` context reads (absent → SQL NULL, matched null-safely).

use super::super::*;

/// Dialect null-safe equality (`negated` → inequality). Unlike plain `=`, a NULL on either
/// side is a real operand: `NULL` matches `NULL`, and `col <=> NULL` is `col IS NULL`. Used
/// where an absent value binds to SQL NULL yet must still match the rows that are themselves
/// unset — an optional `$ctx.field?` read whose field is absent (auth.md Handle 1).
pub(super) fn nullsafe_cmp(dialect: Dialect, lhs: &str, rhs: &str, negated: bool) -> String {
    let eq = match dialect {
        Dialect::Sqlite => format!("{lhs} IS {rhs}"),
        Dialect::MariaDb | Dialect::MySql => format!("{lhs} <=> {rhs}"),
        Dialect::Postgres => format!("{lhs} IS NOT DISTINCT FROM {rhs}"),
    };
    if negated {
        format!("NOT ({eq})")
    } else {
        eq
    }
}

/// A `$ctx.<field>?` optional context read (auth.md Handle 1) — an absent field binds SQL
/// NULL. Distinct from an optional `?` filter param (a marker on the signature).
pub(super) fn is_optional_ctx_read(v: &Value) -> bool {
    matches!(v, Value::Param(pr) if pr.optional && pr.name.node == "ctx" && pr.path.len() == 1)
}

/// Wrap a param's predicate in its optional-filter guard. A non-optional param's predicate is
/// used verbatim; a `?` optional param (queries.md) is guarded by a companion `:p__present`
/// flag the runtime binds — `0` when the arg is absent (the whole clause is a no-op, so the
/// filter drops), `1` when supplied. Operator-agnostic: an absent arg widens the leaf to TRUE,
/// so it composes correctly through `and`/`or` for `~`, ranges, `in`, `has`, or equality.
pub(super) fn present_guard(p: &Param, predicate: &str) -> String {
    if !p.optional {
        return predicate.to_string();
    }
    let present = format!(":{}__present", p.name.node);
    format!("({present} = 0 OR {predicate})")
}

impl<'a> Select<'a> {
    /// Wrap a body predicate in its optional-filter present-guard when its RHS is a `?` param.
    /// An absent arg sets `:{name}__present` = 0, widening the leaf to TRUE so the filter drops
    /// and composes through `and`/`or` (queries.md). A non-optional RHS is returned verbatim.
    pub(super) fn guard_optional(&self, value: &Value, pred: String) -> String {
        if let Value::Param(pr) = value {
            if pr.path.is_empty() && self.optional_params.contains(pr.name.node.as_str()) {
                return format!("(:{}__present = 0 OR {pred})", pr.name.node);
            }
        }
        pred
    }
}
