use super::*;

/// Validate a param's binding + default against the target model. When `infer` is
/// set (bare/inline query), an unbound param must name a same-named column.
/// Validate every `?` optional filter param on a query. An optional filter works
/// with any operator, is list/aggregate-only, and must not also carry a default. It applies to a
/// signature param (bare/inline) or a `$`-referenced comparison in a block `where` — both are
/// present-guarded — but not to a raw body, whose SQL is verbatim. Each violation is its own
/// diagnostic; the param is otherwise checked normally.
pub(super) fn check_optional_params(q: &Query, verb: Verb, sink: &mut Sink) {
    for p in q.params.iter().filter(|p| p.optional) {
        if matches!(&q.body, QueryBody::Raw(_)) {
            sink.error_note(
                code::OPT_PARAM_UNFILTERED,
                p.name.span,
                format!("`?` on param `{}` has no effect in a raw query", p.name.node),
                "a raw query's SQL is verbatim, so a `?` param can't be present-guarded; write the optionality into the raw SQL or use a non-raw query",
            );
            continue;
        }
        if matches!(verb, Verb::Get) {
            sink.error_note(
                code::OPT_PARAM_GET,
                p.name.span,
                format!(
                    "`?` optional filter on param `{}` needs a list query",
                    p.name.node
                ),
                "a `get` is keyed on a unique field; return `[]` (a list), or drop the `?`",
            );
        }
        if p.default.is_some() {
            sink.error_note(
                code::OPT_PARAM_DEFAULT,
                p.name.span,
                format!("param `{}` is both optional (`?`) and defaulted", p.name.node),
                "skip-when-absent and fill-when-absent contradict — keep the `?` or the `= default`, not both",
            );
        }
    }
}
